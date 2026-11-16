use super::{policy::{self,Rule},store,service};
use crate::{model::{Credentials,Request,Result},win::{self,wide}};
use serde::{Deserialize,Serialize};
use serde_json::{Value,json};
use std::{fs,mem::zeroed,path::{Path,PathBuf},ptr::{null,null_mut},time::{Duration,Instant}};
use windows_sys::Win32::{Foundation::*,System::Services::*};
use windows::{core::{HSTRING,Interface},Win32::{System::Com::{CoInitializeEx,CoCreateInstance,CLSCTX_INPROC_SERVER,COINIT_MULTITHREADED,IPersistFile},UI::Shell::{IShellLinkW,ShellLink}}};

#[derive(Clone,Debug,Default,Serialize,Deserialize)]
#[serde(default,rename_all="camelCase")]
pub struct Options{
    pub user_account:String,
    pub user_sid:String,
    pub candidate:Option<Rule>,
    pub rule_id:String,
    pub trusted:bool,
    pub update_identity:bool,
    pub name_filter:String,
}
struct Sc(SC_HANDLE);
impl Drop for Sc{fn drop(&mut self){unsafe{CloseServiceHandle(self.0);}}}
pub fn installed()->Result<bool>{
    let scm=unsafe{OpenSCManagerW(null(),null(),SC_MANAGER_CONNECT)};
    if scm.is_null(){return Err(win::error("打开服务管理器失败"));}
    let scm=Sc(scm);let service=unsafe{OpenServiceW(scm.0,wide(super::SERVICE).as_ptr(),SERVICE_QUERY_STATUS)};
    if service.is_null(){if unsafe{GetLastError()}==ERROR_SERVICE_DOES_NOT_EXIST{return Ok(false);}return Err(win::error("查询授权服务失败"));}
    let _service=Sc(service);Ok(true)
}
fn ensure_backend()->Result<()>{
    use std::os::windows::fs::OpenOptionsExt;
    store::initialize()?;
    let _lock=win::io(fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).share_mode(0).open(store::directory().join("upgrade.lock")))
        .map_err(|_|"另一个任务正在初始化或更新授权后台，请稍后重试。")?;
    let directory=store::install_directory();store::protected_directory(&directory,true)?;
    let executable=directory.join("ApprovedLauncherRust.exe");let source=win::current_exe()?;
    let source_hash=crate::remote::hash_file(&source)?;
    let mut needs_update=!executable.exists()||crate::remote::hash_file(&executable)?!=source_hash;
    let staged=directory.join(format!("upgrade-{}.exe",crate::model::new_id()));
    struct Staged(PathBuf);impl Drop for Staged{fn drop(&mut self){let _=fs::remove_file(&self.0);}}
    let _staged=Staged(staged.clone());
    if needs_update{
        let mut output=win::io(fs::OpenOptions::new().write(true).create_new(true).open(&staged))?;
        win::io(std::io::copy(&mut win::io(fs::File::open(&source))?,&mut output))?;win::io(output.sync_all())?;drop(output);
        if crate::remote::hash_file(&staged)?!=source_hash{return Err("授权后台复制校验失败，原文件保持不变。".into());}
    }
    policy::inspect_storage(&directory)?;
    unsafe{
        let scm=OpenSCManagerW(null(),null(),SC_MANAGER_CONNECT|SC_MANAGER_CREATE_SERVICE);
        if scm.is_null(){return Err(win::error("初始化授权后台失败"));}let scm=Sc(scm);
        let rights=SERVICE_START|SERVICE_STOP|SERVICE_QUERY_STATUS|SERVICE_QUERY_CONFIG;
        let mut handle=OpenServiceW(scm.0,wide(super::SERVICE).as_ptr(),rights);
        if handle.is_null(){
            if GetLastError()!=ERROR_SERVICE_DOES_NOT_EXIST{return Err(win::error("打开授权服务失败"));}
            let command=format!("\"{}\" --approved-service",executable.display());
            handle=CreateServiceW(scm.0,wide(super::SERVICE).as_ptr(),wide("Approved Launcher Rust").as_ptr(),rights,
                SERVICE_WIN32_OWN_PROCESS,SERVICE_AUTO_START,SERVICE_ERROR_NORMAL,wide(&command).as_ptr(),null(),null_mut(),null(),null(),null());
            if handle.is_null(){return Err(win::error("创建授权服务失败"));}
        }
        let service=Sc(handle);let mut required=0;QueryServiceConfigW(service.0,null_mut(),0,&mut required);
        if required==0||required>65536{return Err("读取授权服务配置失败。".into());}
        let mut config=vec![0u64;(required as usize).div_ceil(8)];
        win::check(QueryServiceConfigW(service.0,config.as_mut_ptr().cast(),required,&mut required),"读取授权服务配置失败")?;
        let config=&*(config.as_ptr() as *const QUERY_SERVICE_CONFIGW);
        let expected=format!("\"{}\" --approved-service",executable.display());
        if !win::string(config.lpBinaryPathName).eq_ignore_ascii_case(&expected)||!win::string(config.lpServiceStartName).eq_ignore_ascii_case("LocalSystem"){
            return Err("同名授权服务的程序或运行身份不符，未替换或启动。".into());
        }
        let mut status:SERVICE_STATUS=zeroed();win::check(QueryServiceStatus(service.0,&mut status),"查询授权后台失败")?;
        let mut backup=None;
        if needs_update{
            if status.dwCurrentState!=SERVICE_STOPPED{
                let mut list=service::Request::default();list.operation="list".into();let current=service::send(&list)?;
                if current["backendRevision"]==super::BACKEND_REVISION{needs_update=false;}
                else{
                    let active=current["activeLaunches"].as_u64().ok_or("旧版授权后台不提供运行计数，无法确认空闲；请由管理员确认相关程序退出并停止 ApprovedLauncherRustBroker 后重试。")?;
                    if active>0{return Err("目标后台仍有授权程序运行，未中断或替换。关闭相关程序后重试。".into());}
                    win::check(ControlService(service.0,SERVICE_CONTROL_STOP,&mut status),"授权后台暂时不能停止，未替换文件")?;
                    wait_state(service.0,SERVICE_STOPPED,20)?;
                }
            }
            if needs_update{
                if executable.exists(){let old=directory.join(format!("previous-{}.exe",crate::model::new_id()));win::io(fs::rename(&executable,&old))?;backup=Some(old);}
                if let Err(error)=win::io(fs::rename(&staged,&executable)){
                    if let Some(old)=&backup{win::io(fs::rename(old,&executable))?;let _=StartServiceW(service.0,0,null());}
                    return Err(error);
                }
            }
        }
        let started=(||->Result<()>{
            if StartServiceW(service.0,0,null())==0&&GetLastError()!=ERROR_SERVICE_ALREADY_RUNNING{return Err(win::error("启动授权后台失败"));}
            wait_state(service.0,SERVICE_RUNNING,15)?;
            let mut hello=service::Request::default();hello.operation="list".into();
            let reply=service::send(&hello)?;
            if needs_update&&reply["backendRevision"]!=super::BACKEND_REVISION{return Err("后台启动后未确认预期修订版本。".into());}
            Ok(())
        })();
        if let Err(error)=started{
            if let Some(old)=backup{
                let mut status=zeroed();let _=ControlService(service.0,SERVICE_CONTROL_STOP,&mut status);wait_state(service.0,SERVICE_STOPPED,20)?;
                let failed=directory.join(format!("failed-{}.exe",crate::model::new_id()));win::io(fs::rename(&executable,&failed))?;win::io(fs::rename(old,&executable))?;
                let restart=StartServiceW(service.0,0,null());let _=fs::remove_file(failed);
                return Err(format!("新版后台启动失败，已恢复旧文件（旧服务启动请求：{}）：{error}",restart!=0));
            }
            return Err(error);
        }
        if let Some(old)=backup{let _=fs::remove_file(old);}
    }
    Ok(())
}
unsafe fn wait_state(service:SC_HANDLE,state:u32,seconds:u64)->Result<()>{unsafe{
    let end=Instant::now()+Duration::from_secs(seconds);
    loop{let mut current:SERVICE_STATUS=zeroed();win::check(QueryServiceStatus(service,&mut current),"查询授权服务状态失败")?;
        if current.dwCurrentState==state{return Ok(());}if Instant::now()>end{return Err("等待授权服务状态超时，请核对后台状态。".into());}
        std::thread::sleep(Duration::from_millis(100));
    }
}}
pub fn desktop(account:&str)->Result<PathBuf>{let path=win::profile_folder(account,"Desktop","Desktop")?;policy::local_path(&path.to_string_lossy())?;Ok(path)}
pub fn preview(source:&str,user:&str)->Result<Rule>{
    policy::local_path(source)?;
    let mut target=source.to_owned();let mut arguments=String::new();
    let mut working=Path::new(source).parent().unwrap().to_string_lossy().into_owned();
    if source.to_ascii_lowercase().ends_with(".lnk"){
        unsafe{
            let _=CoInitializeEx(None,COINIT_MULTITHREADED);
            let link:IShellLinkW=CoCreateInstance(&ShellLink,None,CLSCTX_INPROC_SERVER).map_err(|e|e.to_string())?;
            let file:IPersistFile=link.cast().map_err(|e|e.to_string())?;
            file.Load(&HSTRING::from(source),windows::Win32::System::Com::STGM_READ).map_err(|e|e.to_string())?;
            let mut buffer=vec![0u16;32768];
            link.GetPath(&mut buffer,std::ptr::null_mut(),0).map_err(|e|e.to_string())?;
            target=String::from_utf16_lossy(&buffer).trim_end_matches('\0').into();
            buffer.fill(0);link.GetArguments(&mut buffer).map_err(|e|e.to_string())?;
            arguments=String::from_utf16_lossy(&buffer).trim_end_matches('\0').into();
            buffer.fill(0);link.GetWorkingDirectory(&mut buffer).map_err(|e|e.to_string())?;
            working=String::from_utf16_lossy(&buffer).trim_end_matches('\0').into();
            if working.is_empty(){working=Path::new(&target).parent().unwrap_or(Path::new("")).to_string_lossy().into_owned();}
        }
    }
    let mut rule=Rule{
        id:crate::model::new_id(),name:Path::new(source).file_stem().unwrap_or_default().to_string_lossy().into_owned(),
        target,target_sha256:String::new(),arguments,working_directory:working,allowed_sids:vec![win::account_sid(user)?],
        enabled:true,trusted_application_confirmed:true,created:json!(crate::tasks::timestamp())
    };
    rule.validate()?;
    let mut target=policy::TargetLease::open(&rule,false)?;rule.target_sha256=target.hash()?;
    Ok(rule)
}
pub fn create_shortcut(rule:&Rule,user:&str)->Result<PathBuf>{
    let desktop=desktop(user)?;if !desktop.is_dir(){return Err("目标用户桌面目录不存在。".into());}
    let name: String=rule.name.chars().map(|c|if "<>:\"/\\|?*".contains(c)||c.is_control(){'_'}else{c}).collect();
    let path=desktop.join(format!("{name}（授权启动）.lnk"));
    if path.exists(){
        crate::postinstall::shortcut(&path,&store::install_directory().join("ApprovedLauncherRust.exe"),&format!("--launch-rule {}",policy::id(&rule.id)?))?;
        return Ok(path);
    }
    unsafe{
        let _=CoInitializeEx(None,COINIT_MULTITHREADED);
        let link:IShellLinkW=CoCreateInstance(&ShellLink,None,CLSCTX_INPROC_SERVER).map_err(|e|e.to_string())?;
        link.SetPath(&HSTRING::from(store::install_directory().join("ApprovedLauncherRust.exe").to_string_lossy().as_ref())).map_err(|e|e.to_string())?;
        link.SetArguments(&HSTRING::from(format!("--launch-rule {}",policy::id(&rule.id)?))).map_err(|e|e.to_string())?;
        link.SetWorkingDirectory(&HSTRING::from(store::install_directory().to_string_lossy().as_ref())).map_err(|e|e.to_string())?;
        link.SetIconLocation(&HSTRING::from(&rule.target),0).map_err(|e|e.to_string())?;
        let file:IPersistFile=link.cast().map_err(|e|e.to_string())?;
        file.Save(&HSTRING::from(path.to_string_lossy().as_ref()),true).map_err(|e|e.to_string())?;
    }
    Ok(path)
}
pub fn execute(request:&Request,credentials:&Credentials)->Result<Value>{
    let operation=request.operation.strip_prefix("elevation-").ok_or("未知授权操作。")?;
    if operation=="create"{
        if !request.elevation.trusted{return Err("需要点击创建按钮明确授权所选软件。".into());}
        let mut approve=request.clone();approve.operation="elevation-approve".into();
        approve.elevation.candidate=Some(match request.elevation.candidate.clone(){
            Some(candidate) if candidate.target.eq_ignore_ascii_case(&request.source)=>candidate,
            _=>preview(&request.source,&request.elevation.user_account)?
        });
        return execute(&approve,credentials);
    }
    if operation=="preview"||operation=="discover"{
        let user=&request.elevation.user_account;
        if user.is_empty(){return Err("请检测并选择目标电脑登录用户。".into());}
        let actual=win::account_sid(user)?;
        if !request.elevation.user_sid.is_empty()&&actual!=request.elevation.user_sid{return Err("目标用户 SID 变化，请重新检测。".into());}
        let candidates=if operation=="preview"{vec![preview(&request.source,user)?]}else{
            let mut paths=Vec::new();
            fn walk(directory:&Path,depth:usize,paths:&mut Vec<PathBuf>){
                if depth==0||paths.len()>3000{return;}
                for entry in fs::read_dir(directory).into_iter().flatten().flatten().take(3000){
                    use std::os::windows::fs::MetadataExt;
                    let Ok(metadata)=fs::symlink_metadata(entry.path())else{continue;};if metadata.file_attributes()&0x400!=0{continue;}
                    if metadata.is_dir(){walk(&entry.path(),depth-1,paths);}
                    else if entry.path().extension().is_some_and(|ext|ext.eq_ignore_ascii_case("lnk")||ext.eq_ignore_ascii_case("exe")){paths.push(entry.path());}
                }
            }
            let public=std::env::var("PUBLIC").unwrap_or("C:\\Users\\Public".into());let program_data=std::env::var("ProgramData").unwrap_or("C:\\ProgramData".into());
            let mut directories=vec![desktop(user)?,PathBuf::from(public).join("Desktop"),PathBuf::from(program_data).join("Microsoft\\Windows\\Start Menu\\Programs")];
            if let Ok(programs)=crate::postinstall::user_folder(user,"Programs","AppData\\Roaming\\Microsoft\\Windows\\Start Menu\\Programs"){directories.push(programs);}
            for directory in directories{walk(&directory,4,&mut paths);}paths.sort();paths.dedup();
            let filter=request.elevation.name_filter.to_lowercase();
            let mut seen=std::collections::HashSet::new();
            paths.into_iter().filter(|path|filter.is_empty()||path.file_stem().is_some_and(|name|name.to_string_lossy().to_lowercase().contains(&filter)))
                .filter_map(|path|preview(&path.to_string_lossy(),user).ok()).filter(|rule|seen.insert(format!("{}\n{}",rule.target.to_lowercase(),rule.arguments))).collect()
        };
        return Ok(json!({"candidates":candidates,"desktop":desktop(user)?,"userAccount":user,"userSid":actual}));
    }
    if operation=="list"&&!installed()?{return Ok(json!({"rules":[],"message":"目标机尚未配置 Rust 授权后台。"}));}
    if operation=="approve"{
        if !request.elevation.trusted{return Err("必须明确确认授权的软件和运行链可信。".into());}
        let mut candidate=request.elevation.candidate.clone().ok_or("请先预览软件目标。")?;
        let user=&request.elevation.user_account;
        if candidate.allowed_sids!=vec![win::account_sid(user)?]{return Err("目标用户与预览不一致。".into());}
        policy::TargetLease::open(&candidate,true)?;
        desktop(user)?;
        ensure_backend()?;
        let mut list=service::Request::default();list.operation="list".into();let listed=service::send(&list)?;
        let configured=listed["account"].as_str().unwrap_or_default();
        let mut reused=false;
        if let Some(previous)=listed["rules"].as_array().into_iter().flatten().filter_map(|value|serde_json::from_value::<Rule>(value.clone()).ok()).find(|rule|
            rule.enabled&&rule.target.eq_ignore_ascii_case(&candidate.target)&&rule.target_sha256==candidate.target_sha256&&rule.arguments==candidate.arguments&&rule.working_directory.eq_ignore_ascii_case(&candidate.working_directory)&&rule.allowed_sids==candidate.allowed_sids){candidate=previous;reused=true;}
        if !configured.is_empty()&&!configured.eq_ignore_ascii_case(&credentials.account)&&!request.elevation.update_identity{
            return Err(format!("运行账号冲突，后台当前配置为 {configured}。请明确选择更新运行账号。"));
        }
        if configured.is_empty()||request.elevation.update_identity{
            let mut configure=service::Request::default();configure.operation="configure".into();
            configure.account=Some(credentials.account.clone());configure.password=Some(credentials.password.clone());
            service::send(&configure)?;
        }
        let mut save=service::Request::default();save.operation="save".into();save.rule=Some(candidate.clone());
        service::send(&save)?;
        match create_shortcut(&candidate,user){
            Ok(path)=>return Ok(json!({"shortcutPath":path,"ruleId":candidate.id,"state":"Succeeded","name":candidate.name,"target":candidate.target,"enabled":candidate.enabled})),
            Err(error)=>{
                if !reused{let mut revoke=service::Request::default();revoke.operation="revoke".into();revoke.rule_id=Some(candidate.id.clone());service::send(&revoke)?;}
                return Err(error);
            }
        }
    }
    let mut command=service::Request::default();
    command.operation=operation.into();
    if operation=="revoke"{command.rule_id=Some(policy::id(&request.elevation.rule_id)?);}
    let mut result=service::send(&command)?;
    if operation=="revoke"{result["ruleId"]=json!(request.elevation.rule_id);}
    Ok(result)
}

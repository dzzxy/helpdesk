use crate::{model::{Request,Result},win};
use serde::{Deserialize,Serialize};
use serde_json::{Value,json};
use sha2::{Digest,Sha256};
use std::{fs,os::windows::process::CommandExt,path::{Path,PathBuf},process::{Command,Stdio},time::{Duration,Instant}};

pub const ARCHIVE_HASH:&str="93f6c3543fdffa02159967decfb887e035b3904a7d9087f3e5d806bbf06c7b78";
pub const BRIDGE_HASH:&str="bae3d7bd1edd14efcc4bd0a9c64083225ec8ef82748ed514090143c49536bffa";
#[derive(Clone,Debug,Default,Serialize,Deserialize)]
#[serde(default,rename_all="camelCase")]
pub struct Options{pub selected_entry:String,pub display_name:String,pub selected_junk:Vec<Value>,pub session_user_sid:String}

pub fn payload_directory()->Result<PathBuf>{
    let executable=win::current_exe()?;
    let directory=executable.parent().unwrap();
    [directory.join("payloads"),directory.join("../../payloads")]
        .into_iter().find(|path|path.join("BCUninstaller.zip").is_file()).ok_or("找不到 BCU 组件目录。".into())
}
pub fn verify_payload(path:&Path,expected:&str)->Result<()>{
    if crate::remote::hash_file(path)?!=expected{return Err(format!("BCU 组件校验失败：{}",path.display()));}
    Ok(())
}
fn prepare(directory:&Path)->Result<PathBuf>{
    verify_payload(&directory.join("BCUninstaller.zip"),ARCHIVE_HASH)?;
    let output=directory.join("bcu");win::io(fs::create_dir(&output))?;
    let file=win::io(fs::File::open(directory.join("BCUninstaller.zip")))?;
    let mut archive=zip::ZipArchive::new(file).map_err(|e|e.to_string())?;
    let mut total=0u64;
    for index in 0..archive.len(){
        let mut file=archive.by_index(index).map_err(|e|e.to_string())?;
        let relative=file.enclosed_name().ok_or("BCU 压缩包路径越界。")?;
        if file.unix_mode().is_some_and(|mode|mode&0o170000==0o120000){return Err("BCU 压缩包含符号链接。".into());}
        total=total.checked_add(file.size()).ok_or("BCU 压缩包大小无效。")?;
        if total>2*1024*1024*1024{return Err("BCU 压缩包过大。".into());}
        let target=output.join(relative);
        if file.is_dir(){win::io(fs::create_dir_all(target))?;}else{
            win::io(fs::create_dir_all(target.parent().unwrap()))?;
            let mut target=win::io(fs::OpenOptions::new().write(true).create_new(true).open(target))?;
            win::io(std::io::copy(&mut file,&mut target))?;
        }
    }
    let console=output.join("win-x64/BCU-console.exe");
    if !console.is_file(){return Err("BCU 压缩包缺少 win-x64/BCU-console.exe。".into());}
    Ok(console)
}
fn run_command(command:&mut Command,directory:&Path,timeout:u32)->Result<i32>{
    use std::os::windows::io::AsRawHandle;
    let output=win::io(fs::OpenOptions::new().create(true).append(true).open(directory.join("bcu-output.log")))?;
    let error=win::io(output.try_clone())?;
    command.stdout(Stdio::from(output)).stderr(Stdio::from(error)).stdin(Stdio::null()).creation_flags(0x08000000);
    let job=crate::admin::Job::new()?;
    let mut child=win::io(command.spawn())?;
    if let Err(error)=job.assign(child.as_raw_handle()){let _=child.kill();let _=child.wait();return Err(error);}
    let deadline=Instant::now()+Duration::from_secs(timeout as u64);
    loop{
        if let Some(status)=win::io(child.try_wait())?{return Ok(status.code().unwrap_or(1));}
        if directory.join("cancel.signal").exists()||Instant::now()>deadline{
            let _=child.kill();let _=child.wait();return Err("BCU 操作被取消或超时。".into());
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}
fn read_xml(path:&Path)->Result<String>{
    let bytes=win::io(fs::read(path))?;
    if bytes.len()>64*1024*1024{return Err("BCU 软件列表过大。".into());}
    if bytes.starts_with(&[0xff,0xfe]){
        Ok(String::from_utf16_lossy(&bytes[2..].chunks_exact(2).map(|b|u16::from_le_bytes([b[0],b[1]])).collect::<Vec<_>>()))
    }else{String::from_utf8(bytes).map(|s|s.trim_start_matches('\u{feff}').into()).map_err(|e|e.to_string())}
}
pub fn scan(directory:&Path,request:&Request)->Result<Value>{
    let console=prepare(directory)?;
    let inventory=directory.join("bcu-inventory.xml");
    let code=run_command(Command::new(console).args(["export",&inventory.to_string_lossy(),"/U","/V"]),directory,request.timeout.min(600))?;
    if code!=0{return Err(format!("BCU 扫描失败，退出码 {code}。"));}
    let applications = parse_inventory(&read_xml(&inventory)?)?;
    let mut result = json!({"applications": applications});
    match win::sessions(None) {
        Ok(sessions) => result["sessions"] = sessions,
        Err(error) => result["sessionWarning"] = json!(error),
    }
    Ok(result)
}
fn parse_inventory(xml:&str)->Result<Vec<Value>>{
    let doc=roxmltree::Document::parse(&xml).map_err(|e|e.to_string())?;
    if !doc.root_element().has_tag_name("ApplicationEntrySerializer"){return Err("BCU 列表根节点无效。".into());}
    let mut applications=Vec::new();
    for entry in doc.descendants().filter(|node|node.has_tag_name("ApplicationUninstallerEntry")){
        let field=|name:&str|entry.children().find(|node|node.has_tag_name(name)).and_then(|node|node.text()).unwrap_or("").to_owned();
        let name=field("DisplayName");if name.trim().is_empty(){continue;}
        let key=format!("{}\n{}\n{}\n{}",name.trim(),field("DisplayVersion"),field("RegistryPath"),field("UninstallString"));
        applications.push(json!({
            "id":format!("{:x}",Sha256::digest(key.as_bytes())),"displayName":name,
            "version":field("DisplayVersion"),"publisher":field("Publisher"),"architecture":if field("Is64Bit").eq_ignore_ascii_case("true"){"64 位"}else if field("Is64Bit").eq_ignore_ascii_case("false"){"32 位"}else{"未知"},
            "uninstallerKind":field("UninstallerKind"),"installLocation":field("InstallLocation"),
            "registryPath":field("RegistryPath"),"estimatedSize":field("EstimatedSize"),
            "protected":field("IsProtected").eq_ignore_ascii_case("true"),"systemComponent":field("SystemComponent").eq_ignore_ascii_case("true"),
            "valid":!entry.children().any(|node|node.has_tag_name("IsValid")&&!node.text().unwrap_or_default().trim().eq_ignore_ascii_case("true")),
            "interactive":!field("UninstallString").trim().is_empty()&&!entry.children().any(|node|node.has_tag_name("Script")||node.has_tag_name("ConditionScript")),
            "entryXml":&xml[entry.range()]
        }));
    }
    applications.sort_by_key(|app|app["displayName"].as_str().unwrap_or_default().to_lowercase());
    Ok(applications)
}
fn escape(text:&str)->String{text.replace('&',"&amp;").replace('<',"&lt;").replace('>',"&gt;").replace('"',"&quot;")}
fn uninstall_filter(xml:&str)->Result<String>{
    if xml.len()>512*1024{return Err("卸载目标 XML 过大。".into());}
    let doc=roxmltree::Document::parse(xml).map_err(|e|e.to_string())?;
    let root=doc.root_element();
    if !root.has_tag_name("ApplicationUninstallerEntry")||root.descendants().any(|node|node.has_tag_name("Script")||node.has_tag_name("ConditionScript")){
        return Err("卸载目标含脚本或格式无效。".into());
    }
    if root.children().any(|node|node.has_tag_name("IsProtected")&&node.text().unwrap_or_default().trim().eq_ignore_ascii_case("true")||node.has_tag_name("IsValid")&&!node.text().unwrap_or_default().trim().eq_ignore_ascii_case("true")){
        return Err("所选软件受保护或卸载程序无效，未执行卸载。".into());
    }
    if root.children().find(|node|node.has_tag_name("DisplayName")).and_then(|node|node.text()).is_none_or(|s|s.trim().is_empty())
        ||root.children().find(|node|node.has_tag_name("UninstallString")).and_then(|node|node.text()).is_none_or(|s|s.trim().is_empty()) {
        return Err("卸载目标缺少名称或交互卸载命令。".into());
    }
    let mut filters=Vec::new();
    for name in ["DisplayName","DisplayVersion","Publisher","RegistryKeyName","UninstallString"]{
        let value=root.children().find(|node|node.has_tag_name(name)).and_then(|node|node.text()).unwrap_or_default().trim();
        if !value.is_empty(){
            filters.push(format!("<FilterCondition><InvertResults>false</InvertResults><ComparisonMethod>Equals</ComparisonMethod><FilterText>{}</FilterText><TargetPropertyId>{name}</TargetPropertyId><Enabled>true</Enabled></FilterCondition>",escape(value)));
        }
    }
    if filters.len()<2{return Err("卸载目标没有足够的唯一属性。".into());}
    Ok(format!("<UninstallList><Filters><Filter><Name>Rust exact selection</Name><Exclude>false</Exclude><ComparisonEntries>{}</ComparisonEntries><Enabled>true</Enabled></Filter></Filters><Enabled>true</Enabled></UninstallList>",filters.join("")))
}
pub fn uninstall(request:&Request,directory:&Path)->Result<Value>{
    let sessions=win::sessions(None)?;
    if !sessions.as_array().unwrap().iter().any(|session|session["sessionId"]==request.session&&session["state"]=="Active"){
        return Err("选定的卸载会话不再活动。".into());
    }
    if request.bcu.session_user_sid.is_empty()||crate::admin::session_user_sid(request.session)?!=request.bcu.session_user_sid{return Err("卸载会话的登录用户已变化，请重新检测用户。".into());}
    let filter=uninstall_filter(&request.bcu.selected_entry)?;
    let document=roxmltree::Document::parse(&request.bcu.selected_entry).map_err(|e|e.to_string())?;
    let selected_name=document.root_element().children().find(|node|node.has_tag_name("DisplayName")).and_then(|node|node.text()).unwrap_or_default().trim();
    if selected_name!=request.bcu.display_name.trim(){return Err("卸载项目名称与选中目标不匹配。".into());}
    if let Err(error)=crate::admin::confirm_action("IT 软件卸载","确认卸载软件",&format!("{}\n卸载结束后由 IT 确认是否清理残留。",request.bcu.display_name),"开始卸载",directory){
        if error.contains("取消"){return Ok(json!({"cancelled":true,"succeeded":false,"details":"用户或 IT 取消卸载，未启动卸载引擎。"}));}return Err(error);
    }
    if crate::admin::session_user_sid(request.session)?!=request.bcu.session_user_sid{return Err("确认期间登录用户已变化，未执行卸载。".into());}
    let console=prepare(directory)?;
    let list=directory.join("selection.bcul");win::io(fs::write(&list,filter))?;
    let bridge=directory.join("CorpInstaller.BcuBridge.exe");verify_payload(&bridge,BRIDGE_HASH)?;
    let result=directory.join("bridge-result.json");
    let started=Instant::now();
    let code=run_command(Command::new(bridge)
        .arg("--bcu-directory").arg(console.parent().unwrap()).arg("--uninstall-list").arg(list).arg("--result").arg(&result),
        directory,request.timeout)?;
    let mut result:Value=serde_json::from_slice(&win::io(fs::read(result))?).map_err(|e|e.to_string())?;
    if !result["succeeded"].is_boolean(){return Err("BCU 卸载回执缺少成功状态。".into());}
    if code!=0 && result["succeeded"]==true {
        result["succeeded"]=json!(false);
        result["error"]=json!(format!("卸载桥接进程异常退出：{code}，请核对目标机。"));
    }
    let remaining=request.timeout.saturating_sub(started.elapsed().as_secs() as u32);
    let verification=(||->Result<bool>{
        if remaining<5{return Err("卸载后剩余时间不足以复扫。".into());}
        let inventory=directory.join("inventory-after-uninstall.xml");
        let code=run_command(Command::new(&console).args(["export",&inventory.to_string_lossy(),"/U","/V"]),directory,remaining.min(600))?;
        if code!=0{return Err(format!("BCU 卸载复扫失败，退出码 {code}。"));}
        let applications=parse_inventory(&read_xml(&inventory)?)?;
        Ok(selected_present(&request.bcu.selected_entry,&applications)?)
    })();
    match verification{
        Ok(true)=>{result["succeeded"]=json!(false);result["error"]=json!("复扫后所选软件的卸载注册项仍然存在，请核对详细输出。");},
        Ok(false) if result["succeeded"]!=true=>{
            result["partialSuccess"]=json!(true);
            result["error"]=json!(format!("卸载注册项已消失，但卸载或残留扫描异常：{}",result["error"].as_str().unwrap_or("未知错误")));
        },
        Ok(false)=>{result["verifiedRemoved"]=json!(true);},
        Err(error)=>{result["succeeded"]=json!(false);result["state"]=json!("Unconfirmed");result["error"]=json!(error);}
    }
    if result["succeeded"]!=true{result["junkItems"]=json!([]);}
    result["selectedEntryXml"]=json!(request.bcu.selected_entry);
    result["sessionUserSid"]=json!(request.bcu.session_user_sid);
    Ok(result)
}
fn selected_present(xml:&str,applications:&[Value])->Result<bool>{
    let document=roxmltree::Document::parse(xml).map_err(|e|e.to_string())?;
    let field=|name:&str|document.root_element().children().find(|node|node.has_tag_name(name)).and_then(|node|node.text()).unwrap_or_default().trim();
    let registry=field("RegistryPath");
    Ok(applications.iter().any(|app|if !registry.is_empty(){app["registryPath"].as_str().unwrap_or_default().eq_ignore_ascii_case(registry)}else{
        app["displayName"].as_str().unwrap_or_default().eq_ignore_ascii_case(field("DisplayName"))&&app["version"].as_str().unwrap_or_default().eq_ignore_ascii_case(field("DisplayVersion"))
    }))
}

#[cfg(test)]mod tests{
    use super::*;
    #[test]fn exact_filter_rejects_scripts_and_ambiguous_targets(){
        for xml in ["<ApplicationUninstallerEntry><DisplayName>A</DisplayName></ApplicationUninstallerEntry>","<ApplicationUninstallerEntry><DisplayName>A</DisplayName><UninstallString>x.exe</UninstallString><Script>bad</Script></ApplicationUninstallerEntry>"]{assert!(uninstall_filter(xml).is_err());}
        let xml=uninstall_filter("<ApplicationUninstallerEntry><DisplayName>A &amp; B</DisplayName><UninstallString>C:\\Uninstall.exe</UninstallString></ApplicationUninstallerEntry>").unwrap();
        let document=roxmltree::Document::parse(&xml).unwrap();
        assert_eq!(document.descendants().filter(|n|n.has_tag_name("FilterCondition")).count(),2);
        assert!(document.descendants().any(|n|n.has_tag_name("FilterText")&&n.text()==Some("A & B")));
    }
    #[test]fn verification_matches_registry_or_name_and_version(){
        let applications=vec![json!({"displayName":"App","version":"2","registryPath":"HKLM\\Software\\Vendor\\App"})];
        assert!(selected_present("<ApplicationUninstallerEntry><RegistryPath>hklm\\software\\vendor\\app</RegistryPath></ApplicationUninstallerEntry>",&applications).unwrap());
        assert!(!selected_present("<ApplicationUninstallerEntry><DisplayName>App</DisplayName><DisplayVersion>1</DisplayVersion></ApplicationUninstallerEntry>",&applications).unwrap());
        assert!(selected_present("<ApplicationUninstallerEntry><DisplayName>App</DisplayName><DisplayVersion>2</DisplayVersion></ApplicationUninstallerEntry>",&applications).unwrap());
    }
}
pub fn host(args:&[String])->Result<()>{
    let manifest=PathBuf::from(crate::worker::expand(args.get(2).ok_or("缺少任务路径。")?));
    let request=crate::worker::read_request(&manifest)?;
    let outcome=uninstall(&request,manifest.parent().unwrap());
    crate::worker::write_result(&manifest.with_file_name("broker-result.json"),&request.id,outcome)
}

use crate::{model::{Credentials, Request, Result}, win::{self, Handle, wide}};
use serde_json::{Value, json};
use std::{ffi::c_void, mem::{size_of, zeroed}, path::Path, ptr::{null, null_mut}, time::{Duration, Instant}};
use windows_sys::Win32::{Foundation::*, Security::{*, Authorization::*}, Storage::FileSystem::{READ_CONTROL, WRITE_DAC}, System::{SystemServices::{SE_GROUP_LOGON_ID, ACCESS_ALLOWED_ACE_TYPE}, Threading::*, Environment::*, JobObjects::*, StationsAndDesktops::*}, UI::{Shell::{PROFILEINFOW, LoadUserProfileW, UnloadUserProfile}}};

pub fn privilege(name: &str) -> Result<()> {
    unsafe {
        let mut token = null_mut(); win::check(OpenProcessToken(GetCurrentProcess(), TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY, &mut token), "打开服务令牌失败")?;
        let token = Handle(token); let mut luid = zeroed();
        win::check(LookupPrivilegeValueW(null(), wide(name).as_ptr(), &mut luid), "查找服务权限失败")?;
        let privileges = TOKEN_PRIVILEGES { PrivilegeCount: 1, Privileges: [LUID_AND_ATTRIBUTES { Luid: luid, Attributes: SE_PRIVILEGE_ENABLED }] };
        SetLastError(0);
        win::check(AdjustTokenPrivileges(token.0, 0, &privileges, 0, null_mut(), null_mut()), "启用服务权限失败")?;
        if GetLastError() != 0 { return Err(win::error("服务缺少所需权限")); }
        Ok(())
    }
}

pub struct Process { pub handle: Handle, thread: Handle, pub id: u32 }
pub(crate) fn session_user_sid(session:u32)->Result<String>{unsafe{
    privilege("SeTcbPrivilege")?;let mut token=null_mut();
    win::check(windows_sys::Win32::System::RemoteDesktop::WTSQueryUserToken(session,&mut token),"无法读取目标会话用户令牌")?;
    let token=Handle(token);crate::approved::identity::user_sid(token.0)
}}
impl Process {
    pub fn resume(&self) -> Result<()> {
        unsafe { if ResumeThread(self.thread.0) == u32::MAX { TerminateProcess(self.handle.0, 1); return Err(win::error("恢复新进程失败")); } }
        Ok(())
    }
}
pub fn spawn_as(token: HANDLE, exe: &str, arguments: &str, directory: &str) -> Result<Process> {
    unsafe {
        let mut env = null_mut(); win::check(CreateEnvironmentBlock(&mut env, token, 0), "创建账号环境失败")?;
        let mut si: STARTUPINFOW = zeroed(); si.cb = size_of::<STARTUPINFOW>() as u32;
        let mut desktop = wide("winsta0\\default"); si.lpDesktop = desktop.as_mut_ptr();
        let mut pi: PROCESS_INFORMATION = zeroed();
        let ok = CreateProcessAsUserW(token, wide(exe).as_ptr(), wide(&format!("\"{exe}\" {arguments}")).as_mut_ptr(), null(), null(), 0,
            CREATE_UNICODE_ENVIRONMENT | CREATE_SUSPENDED, env, wide(directory).as_ptr(), &si, &mut pi);
        DestroyEnvironmentBlock(env); win::check(ok, "无法在目标会话启动进程")?;
        let process = Handle(pi.hProcess); let thread = Handle(pi.hThread);
        // Assign a job before resuming so the timeout boundary covers every descendant.
        Ok(Process { handle: process, thread, id: pi.dwProcessId })
    }
}

pub fn system_broker(request: &Request, manifest: &Path) -> Result<Process> {
    privilege("SeTcbPrivilege")?; privilege("SeAssignPrimaryTokenPrivilege")?; privilege("SeIncreaseQuotaPrivilege")?;
    unsafe {
        let mut original = null_mut(); win::check(OpenProcessToken(GetCurrentProcess(), TOKEN_DUPLICATE | TOKEN_QUERY, &mut original), "打开 SYSTEM 令牌失败")?;
        let original = Handle(original); let mut token = null_mut();
        win::check(DuplicateTokenEx(original.0, TOKEN_ALL_ACCESS, null(), SecurityImpersonation, TokenPrimary, &mut token), "复制会话令牌失败")?;
        let token = Handle(token); win::set_session(token.0, request.session)?;
        let mode=if request.operation=="bcu-uninstall"{"--bcu-host"}else{"--broker"};
        spawn_as(token.0, &win::current_exe()?.to_string_lossy(), &format!("{mode} \"{}\"", manifest.display()), &manifest.parent().unwrap().to_string_lossy())
    }
}

struct Grant { handle: HANDLE, station: bool, sid: Vec<u8>, mask: u32 }
fn logon_sid(token: HANDLE) -> Result<Vec<u8>> {
    unsafe {
        let mut bytes = 0; GetTokenInformation(token, TokenGroups, null_mut(), 0, &mut bytes);
        let mut buffer = vec![0u64; (bytes as usize).div_ceil(8)];
        win::check(GetTokenInformation(token, TokenGroups, buffer.as_mut_ptr().cast(), bytes, &mut bytes), "读取登录 SID 失败")?;
        let groups = &*(buffer.as_ptr().cast::<TOKEN_GROUPS>());
        for group in std::slice::from_raw_parts(groups.Groups.as_ptr(), groups.GroupCount as usize) {
            if group.Attributes & SE_GROUP_LOGON_ID as u32 == SE_GROUP_LOGON_ID as u32 {
                let mut sid = vec![0; GetLengthSid(group.Sid) as usize];
                win::check(CopySid(sid.len() as u32, sid.as_mut_ptr().cast(), group.Sid), "复制登录 SID 失败")?; return Ok(sid);
            }
        }
        Err("未找到本次登录 SID。".into())
    }
}
impl Grant {
    unsafe fn add(handle: HANDLE, station: bool, sid: &[u8], mask: u32) -> Result<Self> {
        unsafe {
            if handle.is_null() { return Err(win::error("打开目标会话桌面失败")); }
            let grant = Self { handle, station, sid: sid.to_vec(), mask };
            let mut sd = null_mut(); let mut old_acl = null_mut();
            let status = GetSecurityInfo(handle, SE_WINDOW_OBJECT, DACL_SECURITY_INFORMATION, null_mut(), null_mut(), &mut old_acl, null_mut(), &mut sd);
            if status != 0 { return Err(format!("读取桌面权限失败：{status}")); }
            if old_acl.is_null() { LocalFree(sd); return Err("桌面权限为空，停止安装。".into()); }
            let access = EXPLICIT_ACCESS_W { grfAccessPermissions: mask, grfAccessMode: GRANT_ACCESS, grfInheritance: 0,
                Trustee: TRUSTEE_W { pMultipleTrustee: null_mut(), MultipleTrusteeOperation: NO_MULTIPLE_TRUSTEE, TrusteeForm: TRUSTEE_IS_SID,
                    TrusteeType: TRUSTEE_IS_USER, ptstrName: grant.sid.as_ptr() as *mut u16 } };
            let mut acl = null_mut(); let status = SetEntriesInAclW(1, &access, old_acl, &mut acl);
            let status = if status == 0 { SetSecurityInfo(handle, SE_WINDOW_OBJECT, DACL_SECURITY_INFORMATION, null_mut(), null_mut(), acl, null()) } else { status };
            if !acl.is_null() { LocalFree(acl.cast()); } LocalFree(sd);
            if status != 0 { return Err(format!("临时授权桌面失败：{status}")); }
            Ok(grant)
        }
    }
}
impl Drop for Grant {
    fn drop(&mut self) {
        unsafe {
            let mut sd = null_mut(); let mut acl = null_mut();
            if GetSecurityInfo(self.handle, SE_WINDOW_OBJECT, DACL_SECURITY_INFORMATION, null_mut(), null_mut(), &mut acl, null_mut(), &mut sd) == 0 && !acl.is_null() {
                let mut info: ACL_SIZE_INFORMATION = zeroed();
                if GetAclInformation(acl, (&mut info as *mut ACL_SIZE_INFORMATION).cast(), size_of::<ACL_SIZE_INFORMATION>() as u32, AclSizeInformation) != 0 {
                    for index in (0..info.AceCount).rev() {
                        let mut ace: *mut c_void = null_mut();
                        if GetAce(acl, index, &mut ace) != 0 {
                            let allowed = &*(ace.cast::<ACCESS_ALLOWED_ACE>());
                            if allowed.Header.AceType == ACCESS_ALLOWED_ACE_TYPE as u8 && allowed.Mask == self.mask
                                && EqualSid((&allowed.SidStart as *const u32).cast_mut().cast(), self.sid.as_mut_ptr().cast()) != 0 { DeleteAce(acl, index); }
                        }
                    }
                    SetSecurityInfo(self.handle, SE_WINDOW_OBJECT, DACL_SECURITY_INFORMATION, null_mut(), null_mut(), acl, null());
                }
            }
            if !sd.is_null() { LocalFree(sd); }
            if self.station { CloseWindowStation(self.handle); } else { CloseDesktop(self.handle); }
        }
    }
}

struct AdminSession { token: Handle, profile: HANDLE, grants: Vec<Grant> }
impl AdminSession {
    fn new(credentials: &Credentials, session: u32) -> Result<Self> {
        for name in ["SeBackupPrivilege","SeRestorePrivilege","SeTcbPrivilege","SeAssignPrimaryTokenPrivilege","SeIncreaseQuotaPrivilege"] { privilege(name)?; }
        let mut token = win::logon(credentials, 2)?;
        unsafe {
            let mut kind = 0u32; let mut returned = 0;
            win::check(GetTokenInformation(token.0, TokenElevationType, (&mut kind as *mut u32).cast(), 4, &mut returned), "读取管理员令牌失败")?;
            if kind == TokenElevationTypeLimited as u32 {
                let mut linked: TOKEN_LINKED_TOKEN = zeroed();
                win::check(GetTokenInformation(token.0, TokenLinkedToken, (&mut linked as *mut TOKEN_LINKED_TOKEN).cast(), size_of::<TOKEN_LINKED_TOKEN>() as u32, &mut returned), "读取完整管理员令牌失败")?;
                token = Handle(linked.LinkedToken);
            }
            let mut elevation = 0u32;
            win::check(GetTokenInformation(token.0, TokenElevation, (&mut elevation as *mut u32).cast(), 4, &mut returned), "检查管理员权限失败")?;
            if elevation == 0 { return Err("安装账号没有完整管理员令牌；未回退为 SYSTEM。".into()); }
            win::set_session(token.0, session)?;
            let user = win::split_account(&credentials.account).0;
            let mut name = wide(&user); let mut info: PROFILEINFOW = zeroed(); info.dwSize = size_of::<PROFILEINFOW>() as u32; info.dwFlags = 1; info.lpUserName = name.as_mut_ptr();
            win::check(LoadUserProfileW(token.0, &mut info), "加载管理员用户配置失败")?;
            let mut result = Self { token, profile: info.hProfile, grants: Vec::new() };
            let sid = logon_sid(result.token.0)?;
            result.grants.push(Grant::add(OpenWindowStationW(wide("winsta0").as_ptr(), 0, READ_CONTROL | WRITE_DAC), true, &sid, 0x37f)?);
            result.grants.push(Grant::add(OpenDesktopW(wide("default").as_ptr(), 0, 0, READ_CONTROL | WRITE_DAC), false, &sid, 0x1ff)?);
            Ok(result)
        }
    }
}
impl Drop for AdminSession { fn drop(&mut self) { self.grants.clear(); unsafe { UnloadUserProfile(self.token.0, self.profile); } } }

pub struct Job(pub Handle);
impl Job {
    pub fn new() -> Result<Self> {
        unsafe {
            let job = CreateJobObjectW(null(), null()); if job.is_null() { return Err(win::error("创建任务进程组失败")); }
            let job = Self(Handle(job));
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = zeroed(); info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            win::check(SetInformationJobObject(job.0.0, JobObjectExtendedLimitInformation, (&info as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(), size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32), "配置任务进程组失败")?;
            Ok(job)
        }
    }
    pub fn assign(&self, process: HANDLE) -> Result<()> { unsafe { win::check(AssignProcessToJobObject(self.0.0, process), "加入任务进程组失败") } }
    fn process_ids(&self)->Result<Vec<u32>>{unsafe{
        let mut buffer=[0usize;1025];
        win::check(QueryInformationJobObject(self.0.0,JobObjectBasicProcessIdList,buffer.as_mut_ptr().cast(),size_of_val(&buffer) as u32,null_mut()),"读取任务进程树失败")?;
        let header=&*buffer.as_ptr().cast::<JOBOBJECT_BASIC_PROCESS_ID_LIST>();let count=header.NumberOfProcessIdsInList as usize;
        if count>1024||header.NumberOfAssignedProcesses as usize!=count{return Err("任务进程树过大或正在变化。".into());}
        Ok(buffer[1..1+count].iter().map(|id|*id as u32).collect())
    }}
    fn installer_pending(&self) -> bool {
        unsafe {
            // The two DWORD header fields occupy one usize on this x64 build.
            let mut ids = [0usize; 257];
            if QueryInformationJobObject(self.0.0, JobObjectBasicProcessIdList, ids.as_mut_ptr().cast(), size_of_val(&ids) as u32, null_mut()) == 0 {
                return true;
            }
            let header = &*ids.as_ptr().cast::<JOBOBJECT_BASIC_PROCESS_ID_LIST>();
            let count = header.NumberOfProcessIdsInList as usize;
            if count > 256 || header.NumberOfAssignedProcesses as usize != count {
                return true;
            }
            ids[1..1 + count].iter().any(|&pid| {
                let handle = Handle(OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid as u32));
                if handle.0.is_null() {
                    // A process that exited during enumeration can be skipped.
                    return GetLastError() != ERROR_INVALID_PARAMETER;
                }
                let mut path = vec![0u16; 32768];
                let mut len = path.len() as u32;
                if QueryFullProcessImageNameW(handle.0, PROCESS_NAME_WIN32, path.as_mut_ptr(), &mut len) == 0 {
                    return true;
                }
                looks_like_installer(&String::from_utf16_lossy(&path[..len as usize]))
            })
        }
    }
    pub fn detach(&self) -> Result<()> {
        unsafe {
            let info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = zeroed();
            win::check(SetInformationJobObject(self.0.0, JobObjectExtendedLimitInformation, (&info as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(), size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32), "无法保留已启动的进程")
        }
    }
}
fn has_window(processes:&[u32])->bool{
    use windows_sys::Win32::UI::WindowsAndMessaging::*;
    struct Query<'a>{processes:&'a[u32],visible:bool}
    unsafe extern "system" fn check(window:HWND,data:LPARAM)->i32{unsafe{
        let query=&mut *(data as *mut Query<'_>);let mut pid=0;GetWindowThreadProcessId(window,&mut pid);
        if query.processes.contains(&pid)&&IsWindowVisible(window)!=0{query.visible=true;return 0;}1
    }}
    let mut query=Query{processes,visible:false};unsafe{EnumWindows(Some(check),(&mut query as *mut Query<'_>) as isize);}query.visible
}

fn looks_like_installer(path: &str) -> bool {
    let path = path.to_ascii_lowercase();
    let name = path.rsplit('\\').next().unwrap_or_default();
    ["setup", "install", "msiexec", "bootstrap", "patch", "deploy"].iter().any(|part| name.contains(part))
        || ["\\corpinstallerrusttemp\\", "\\appdata\\local\\temp\\", "\\windows\\temp\\"].iter().any(|part| path.contains(part))
}

#[cfg(test)]
#[test]
fn install_process_classification() {
    assert!(!looks_like_installer(r"C:\Program Files\Netease\MuMu\MuMuPlayer.exe"));
    assert!(looks_like_installer(r"C:\Program Files\Netease\MuMuSetup.EXE"));
    assert!(looks_like_installer(r"C:\Users\Tester\AppData\Local\Temp\helper.exe"));
}

pub(crate) fn confirm_action(title:&str,heading:&str,detail:&str,action:&str,directory:&Path)->Result<()> {
    use windows_sys::Win32::UI::{Controls::*, WindowsAndMessaging::*};
    unsafe extern "system" fn check_cancel(window: HWND, notification: u32, _: WPARAM, _: LPARAM, data: isize) -> i32 {
        if notification == TDN_TIMER as u32 && unsafe { &*(data as *const std::path::PathBuf) }.exists() {
            unsafe { PostMessageW(window, TDM_CLICK_BUTTON as u32, IDCANCEL as usize, 0); }
        }
        0
    }
    let cancel = directory.join("cancel.signal");
    if cancel.exists() { return Err("操作已取消。".into()); }
    let title = wide(title);
    let heading = wide(heading);
    let detail = wide(detail);
    let next = wide(action);
    let button = TASKDIALOG_BUTTON { nButtonID: IDOK, pszButtonText: next.as_ptr() };
    let dialog = TASKDIALOGCONFIG {
        cbSize: size_of::<TASKDIALOGCONFIG>() as u32,
        dwFlags: TDF_ALLOW_DIALOG_CANCELLATION | TDF_CALLBACK_TIMER,
        dwCommonButtons: TDCBF_CANCEL_BUTTON,
        pszWindowTitle: title.as_ptr(),
        pszMainInstruction: heading.as_ptr(),
        pszContent: detail.as_ptr(),
        cButtons: 1,
        pButtons: &button,
        nDefaultButton: IDCANCEL,
        pfCallback: Some(check_cancel),
        lpCallbackData: (&cancel as *const std::path::PathBuf) as isize,
        ..Default::default()
    };
    let mut selected = 0;
    let result = unsafe { TaskDialogIndirect(&dialog, &mut selected, null_mut(), null_mut()) };
    if result < 0 { return Err(format!("打开安装向导失败（HRESULT {result:#x}）。")); }
    if selected != IDOK || cancel.exists() { return Err("用户取消或 IT 已取消操作。".into()); }
    Ok(())
}
fn progress(request:&Request,manifest:&Path,phase:&str,detail:&str){
    let temporary=manifest.with_file_name("progress.json.new");let path=manifest.with_file_name("progress.json");
    let data=json!({"id":request.id,"progressKind":"Stage","phase":phase,"detail":detail});
    if let Ok(bytes)=serde_json::to_vec(&data){if std::fs::write(&temporary,bytes).is_ok(){let _=std::fs::rename(temporary,path);}}
}

pub fn install(request: &Request, credentials: &Credentials, manifest: &Path, mut acknowledge: impl FnMut() -> Result<()>) -> Result<Value> {
    let sessions = win::sessions(None)?;
    if !sessions.as_array().unwrap().iter().any(|s|s["sessionId"] == request.session && s["state"] == "Active") { return Err("所选会话已不再活动，请重新检测。".into()); }
    let session = AdminSession::new(credentials, request.session)?;
    let package = crate::worker::expand(&request.source);
    if !Path::new(&package).is_file() { return Err("目标安装文件不存在。".into()); }
    let verify_start = Instant::now();
    if !request.sha256.is_empty() && crate::remote::hash_file(Path::new(&package))? != request.sha256 { return Err("安装文件 SHA-256 校验失败，未执行。".into()); }
    let verify_ms = verify_start.elapsed().as_millis() as u64;
    let mut wizard=if request.operation=="install"{Some(crate::install_wizard::Wizard::open(request,Path::new(&package),manifest.parent().unwrap())?)}else{None};
    if let Some(wizard)=wizard.as_mut(){
        acknowledge()?;
        progress(request,manifest,"等待用户","等待目标电脑用户在 IT 软件安装向导中点击下一步。");
        if !wizard.wait_for_confirmation(Duration::from_secs(request.timeout as u64))?{
            return Ok(json!({"cancelled":true,"state":"Cancelled","exitCode":1602,"details":"用户或管理员取消安装，未启动安装器。"}));
        }
        crate::win::io(std::fs::write(manifest.with_file_name("install-confirmed.signal"), b"confirmed"))?;
    }
    let (exe, args) = if package.to_lowercase().ends_with(".msi") {
        (format!("{}\\System32\\msiexec.exe", std::env::var("SystemRoot").unwrap_or("C:\\Windows".into())), crate::install_options::msi_arguments(&package,&request.arguments))
    } else { (package.clone(), request.arguments.clone()) };
    let snapshot=(request.operation=="install"&&(request.install.start_menu||request.install.desktop||request.install.elevated_shortcut)).then(||crate::postinstall::Snapshot::take(credentials));
    let job = Job::new()?;
    let process = spawn_as(session.token.0, &exe, &args, &Path::new(&package).parent().unwrap().to_string_lossy())?;
    if let Err(e) = job.assign(process.handle.0) { unsafe { TerminateProcess(process.handle.0, 1); } return Err(e); }
    process.resume()?;
    progress(request,manifest,"执行中","安装器正在目标桌面运行，请按目标机安装界面完成操作。");
    let start = Instant::now(); let timeout = Duration::from_secs(if request.operation == "run" { 10 } else { request.timeout } as u64);
    if request.operation != "install" { acknowledge()?; }
    let mut installer_exit_at = None;
    let mut last_wizard_elapsed = None;
    let mut window_seen=false;
    loop {
        if wizard.as_mut().is_some_and(|wizard|wizard.pump()){
            unsafe{TerminateProcess(process.handle.0,1223);}
            return Ok(json!({"cancelled":true,"state":"Cancelled","exitCode":1602,"details":"用户或管理员取消了安装。"}));
        }
        if request.operation=="run"{
            if manifest.parent().unwrap().join("cancel.signal").exists(){return Ok(json!({"cancelled":true,"state":"Cancelled","details":"已取消运行观察，正在结束本次进程组。"}));}
            let ids=match job.process_ids(){Ok(ids)=>ids,Err(error)=>{job.detach()?;return Ok(json!({"state":"Unconfirmed","pid":process.id,"details":error}));}};
            window_seen|=has_window(&ids);let mut exit=0;unsafe{win::check(GetExitCodeProcess(process.handle.0,&mut exit),"读取程序状态失败")?;}
            if start.elapsed()>=Duration::from_secs(10){
                let (state,detail)=if !ids.is_empty(){if window_seen{("WindowShown","已启动，检测到软件窗口")}else{("Started","进程已启动，未检测到窗口；请确认软件界面或托盘状态")}}
                    else if exit!=0&&exit!=STILL_ACTIVE as u32{("Failed","程序已退出，未检测到存活子进程，请核对退出码")}
                    else{("Exited","程序快速退出，未确认软件持续运行")};
                job.detach()?;return Ok(json!({"state":state,"succeeded":state!="Failed","pid":process.id,"sessionId":request.session,"identity":request.account,"windowShown":window_seen,"runningCount":ids.len(),"exitCode":if exit==STILL_ACTIVE as u32{None}else{Some(exit)},"details":detail}));
            }
            std::thread::sleep(Duration::from_millis(200));continue;
        }
        let wait = unsafe { WaitForSingleObject(process.handle.0, 200) };
        if wait == WAIT_FAILED { return Err(win::error("等待安装进程失败")); }
        if wait == WAIT_TIMEOUT {
            let elapsed = start.elapsed().as_secs();
            if last_wizard_elapsed != Some(elapsed) {
                last_wizard_elapsed = Some(elapsed);
                if let Some(wizard) = &wizard {
                    wizard.status(&format!("安装程序正在运行，已用 {} 分 {:02} 秒。\r\n安装进度以软件自身的安装窗口为准。", elapsed / 60, elapsed % 60));
                }
            }
        }
        if wait == WAIT_OBJECT_0 {
            if installer_exit_at.is_none(){
                let detail = "主安装器已退出，正在检查子进程及安装结果。";
                progress(request,manifest,"检查结果",detail);
                if let Some(wizard) = &wizard { wizard.status(detail); }
            }
            let mut exit = 0;
            unsafe { win::check(GetExitCodeProcess(process.handle.0, &mut exit), "读取安装器退出码失败")?; }
            if request.operation=="install"&&exit==1602{
                if let Some(wizard)=wizard.as_mut(){wizard.finish("安装已取消。");}
                return Ok(json!({"cancelled":true,"state":"Cancelled","exitCode":exit,"details":"安装器报告用户取消。"}));
            }
            if request.operation=="install"&&![0,1641,3010].contains(&exit){
                if let Some(wizard)=wizard.as_mut(){wizard.finish(&format!("安装失败，退出代码：{exit}"));}
                unsafe{TerminateJobObject(job.0.0,exit);}
                let rollback=rollback(request,session.token.0,&package);
                return Ok(json!({"succeeded":false,"state":"Failed","exitCode":exit,"error":format!("安装器退出码 {exit}，请核对安装结果。"),"rollback":rollback}));
            }
            let elapsed = installer_exit_at.get_or_insert_with(Instant::now).elapsed();
            let no_children = job.process_ids().is_ok_and(|ids| ids.is_empty());
            let state = if request.operation == "run" {
                Some("Exited")
            } else if no_children || (elapsed >= Duration::from_secs(5) && !job.installer_pending()) {
                Some(if exit == 3010 || exit == 1641 { "RebootRequired" } else { "Succeeded" })
            } else if elapsed >= Duration::from_secs(30) || start.elapsed() >= timeout {
                Some("Unconfirmed")
            } else {
                None
            };
            if let Some(state) = state {
                job.detach()?;
                let details = if state == "Unconfirmed" { "主安装器已退出，仍有疑似安装进程或无法识别的进程，请在目标机核对。" } else { "" };
                let post_install=if state!="Unconfirmed"{
                    if snapshot.is_some() {
                        let detail = "安装器已退出，正在识别软件入口并处理快捷方式或授权。";
                        progress(request,manifest,"安装后处理",detail);
                        if let Some(wizard) = &wizard { wizard.status(detail); }
                    }
                    let user=sessions.as_array().into_iter().flatten().find(|row|row["sessionId"]==request.session).and_then(|row|row["account"].as_str()).unwrap_or_default();
                    snapshot.as_ref().map(|snapshot|crate::postinstall::complete(snapshot,request,credentials,user,manifest.parent().unwrap()))
                }else{None};
                if let Some(wizard)=wizard.as_mut(){wizard.finish(if post_install.as_ref().is_some_and(|result|result["succeeded"]==false){"安装器已完成，快捷方式或授权处理需要 IT 核对。"}else{match state{"Unconfirmed"=>"主安装器已退出，结果待 IT 核对。","RebootRequired"=>"安装成功，需要重启电脑后生效。",_=>"安装成功。"}});}
                let installer_runtime = installer_exit_at.unwrap().duration_since(start);
                return Ok(json!({
                    "state":state,"exitCode":exit,"pid":process.id,"identity":request.account,"sessionId":request.session,"details":details,
                    "succeeded":post_install.as_ref().is_none_or(|result|result["succeeded"]!=false),"partialSuccess":post_install.as_ref().is_some_and(|result|result["succeeded"]==false),"cancelled":post_install.as_ref().is_some_and(|result|result["cancelled"]==true),
                    "postInstall":post_install,"目标机包校验毫秒":verify_ms,"主安装器运行毫秒":installer_runtime.as_millis() as u64,"退出后观察毫秒":elapsed.as_millis() as u64
                }));
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        if manifest.parent().unwrap().join("cancel.signal").exists() { unsafe { TerminateProcess(process.handle.0, 1223); } return Err("操作已取消。".into()); }
        if start.elapsed() > timeout {
            if request.operation == "run" { job.detach()?; return Ok(json!({"state":"Started","pid":process.id,"identity":request.account,"sessionId":request.session,"details":"进程运行超过 10 秒；窗口可见性及应用功能请在目标机核对。"})); }
            unsafe { TerminateJobObject(job.0.0,1460); }
            return Ok(json!({"succeeded":false,"state":"Failed","error":"安装器执行超时，已请求结束；请核对目标机。","rollback":rollback(request,session.token.0,&package)}));
        }
    }
}
fn rollback(request:&Request,token:HANDLE,package:&str)->Value{
    if !request.install.rollback{return json!({"requested":false});}
    let outcome=(||->Result<u32>{
        let command=if request.install.rollback_command=="{package}"{package.to_owned()}else{win::expand_environment(&request.install.rollback_command)?};
        crate::model::drive_path(&command)?;if !Path::new(&command).is_file(){return Err("回滚程序不存在。".into());}
        let (executable,arguments)=if command.to_ascii_lowercase().ends_with(".msi"){
            (format!("{}\\System32\\msiexec.exe",std::env::var("SystemRoot").unwrap_or("C:\\Windows".into())),format!("/i \"{command}\" {}",request.install.rollback_arguments))
        }else{(command.clone(),request.install.rollback_arguments.clone())};
        let job=Job::new()?;let process=spawn_as(token,&executable,&arguments,&Path::new(&command).parent().ok_or("回滚目录无效。")?.to_string_lossy())?;
        if let Err(error)=job.assign(process.handle.0){unsafe{TerminateProcess(process.handle.0,1);}return Err(error);}
        process.resume()?;
        if unsafe{WaitForSingleObject(process.handle.0,request.timeout.clamp(10,600)*1000)}!=WAIT_OBJECT_0{return Err("回滚取消或超时，进程组将结束，请核对目标状态。".into());}
        let mut code=0;unsafe{win::check(GetExitCodeProcess(process.handle.0,&mut code),"读取回滚退出码失败")?;}
        Ok(code)
    })();
    match outcome{Ok(code)=>json!({"requested":true,"succeeded":([0,1641,3010].contains(&code)),"exitCode":code}),Err(error)=>json!({"requested":true,"succeeded":false,"error":error})}
}



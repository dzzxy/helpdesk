use crate::model::{Credentials, Result};
use serde_json::{Value, json};
use std::{ffi::c_void, ptr::{null, null_mut}};
use windows_sys::Win32::{Foundation::*, Security::*, System::{RemoteDesktop::*, Registry::*}};
use zeroize::Zeroizing;

pub fn wide(s: &str) -> Vec<u16> { s.encode_utf16().chain(Some(0)).collect() }
pub unsafe fn string(p: *const u16) -> String {
    if p.is_null() { return String::new(); }
    let mut len = 0; unsafe { while *p.add(len) != 0 { len += 1; } String::from_utf16_lossy(std::slice::from_raw_parts(p, len)) }
}
pub fn error(context: &str) -> String { format!("{context}：{}", std::io::Error::last_os_error()) }
pub fn check(value: i32, context: &str) -> Result<()> { if value == 0 { Err(error(context)) } else { Ok(()) } }
pub fn message(title: &str, text: &str) { unsafe { windows_sys::Win32::UI::WindowsAndMessaging::MessageBoxW(null_mut(), wide(text).as_ptr(), wide(title).as_ptr(), 0x10); } }

pub struct Handle(pub HANDLE);
impl Drop for Handle { fn drop(&mut self) { if !self.0.is_null() && self.0 != INVALID_HANDLE_VALUE { unsafe { CloseHandle(self.0); } } } }

pub fn split_account(account: &str) -> (String, Option<String>) {
    if let Some((domain, user)) = account.split_once('\\') { (user.into(), Some(domain.into())) } else { (account.into(), None) }
}
pub fn logon(credentials: &Credentials, logon_type: u32) -> Result<Handle> {
    let (user, domain) = split_account(&credentials.account);
    let user = wide(&user); let domain = domain.map(|d| wide(&d));
    let password = Zeroizing::new(wide(&credentials.password));
    let mut token = null_mut();
    unsafe { check(LogonUserW(user.as_ptr(), domain.as_ref().map_or(null(), |d| d.as_ptr()), password.as_ptr(), logon_type, if logon_type == 9 { 3 } else { 0 }, &mut token), "Windows 账号登录失败")?; }
    Ok(Handle(token))
}
pub struct NetworkIdentity { _token: Handle, previous:Option<Handle>, _thread:std::marker::PhantomData<std::rc::Rc<()>> }
impl NetworkIdentity {
    pub fn new(credentials: &Credentials) -> Result<Self> {
        let token = logon(credentials, 9)?;
        let previous=unsafe{
            let mut previous=null_mut();
            if windows_sys::Win32::System::Threading::OpenThreadToken(windows_sys::Win32::System::Threading::GetCurrentThread(),TOKEN_IMPERSONATE|TOKEN_QUERY|TOKEN_DUPLICATE,1,&mut previous)!=0{Some(Handle(previous))}
            else if GetLastError()==ERROR_NO_TOKEN{None}else{return Err(error("读取原出站身份失败"));}
        };
        unsafe { check(ImpersonateLoggedOnUser(token.0), "无法隔离出站凭据")?; }
        Ok(Self { _token: token, previous, _thread:Default::default() })
    }
}
impl Drop for NetworkIdentity { fn drop(&mut self) { unsafe { if let Some(previous)=&self.previous{windows_sys::Win32::System::Threading::SetThreadToken(null(),previous.0);}else{RevertToSelf();} } } }

pub fn sessions(computer: Option<&str>) -> Result<Value> {
    unsafe {
        let server = match computer {
            Some(h) => { let server = WTSOpenServerW(wide(h).as_ptr()); if server.is_null() { return Err(error("无法连接会话查询接口")); } server },
            None => null_mut(),
        };
        let mut data = null_mut(); let mut count = 0;
        let ok = WTSEnumerateSessionsW(server, 0, 1, &mut data, &mut count);
        if ok == 0 { if !server.is_null() { WTSCloseServer(server); } return Err(error("读取会话失败")); }
        let mut rows = Vec::new();
        for info in std::slice::from_raw_parts(data, count as usize) {
            let user = session_string(server, info.SessionId, WTSUserName);
            if user.is_empty() { continue; }
            let domain = session_string(server, info.SessionId, WTSDomainName);
            rows.push(json!({"sessionId": info.SessionId, "account": format!("{domain}\\{user}"),
                "state": if info.State == WTSActive { "Active" } else { "Disconnected/Other" },
                "station": string(info.pWinStationName), "client": session_string(server, info.SessionId, WTSClientName)}));
        }
        WTSFreeMemory(data.cast()); if !server.is_null() { WTSCloseServer(server); }
        Ok(json!(rows))
    }
}
unsafe fn session_string(server: HANDLE, session: u32, class: WTS_INFO_CLASS) -> String {
    unsafe {
        let mut buffer = null_mut(); let mut len = 0;
        if WTSQuerySessionInformationW(server, session, class, &mut buffer, &mut len) == 0 { return String::new(); }
        let value = string(buffer); WTSFreeMemory(buffer.cast()); value
    }
}

pub struct Reg(pub HKEY);
impl Drop for Reg { fn drop(&mut self) { unsafe { RegCloseKey(self.0); } } }
pub fn reg_open(root: HKEY, key: &str, write: bool) -> Result<Reg> {
    reg_open_view(root, key, write, KEY_WOW64_64KEY)
}
fn reg_open_view(root: HKEY, key: &str, write: bool, view: u32) -> Result<Reg> {
    let mut handle = null_mut();
    let status = unsafe { RegOpenKeyExW(root, wide(key).as_ptr(), 0, KEY_READ | view | if write { KEY_SET_VALUE } else { 0 }, &mut handle) };
    if status != 0 { return Err(format!("无法打开注册表 {key}：{}", std::io::Error::from_raw_os_error(status as i32))); }
    Ok(Reg(handle))
}
pub fn reg_string(root: HKEY, key: &str, name: &str) -> Result<String> {
    let key = reg_open(root, key, false)?;
    read_reg_value(key.0, name)
}
fn read_reg_value(key: HKEY, name: &str) -> Result<String> {
    let mut size = 0; let mut kind = 0;
    unsafe {
        let status = RegQueryValueExW(key, wide(name).as_ptr(), null(), &mut kind, null_mut(), &mut size);
        if status != 0 { return Err(format!("读取 {name} 失败：{status}")); }
        if size > 1024 * 1024 { return Err("注册表值过大。".into()); }
        let mut data = vec![0u8; size as usize + 2];
        let status = RegQueryValueExW(key, wide(name).as_ptr(), null(), &mut kind, data.as_mut_ptr(), &mut size);
        if status != 0 { return Err(format!("读取 {name} 失败：{status}")); }
        Ok(format_reg(kind, &data[..size as usize]))
    }
}
fn format_reg(kind: u32, bytes: &[u8]) -> String {
    match kind {
        REG_SZ | REG_EXPAND_SZ | REG_MULTI_SZ => String::from_utf16_lossy(&bytes.chunks_exact(2).map(|b| u16::from_le_bytes([b[0],b[1]])).collect::<Vec<_>>()).trim_end_matches('\0').replace('\0', " | "),
        REG_DWORD if bytes.len() >= 4 => u32::from_le_bytes(bytes[..4].try_into().unwrap()).to_string(),
        REG_QWORD if bytes.len() >= 8 => u64::from_le_bytes(bytes[..8].try_into().unwrap()).to_string(),
        _ => bytes.iter().take(256).map(|b| format!("{b:02X} ")).collect(),
    }
}
pub fn registry_browse(path: &str, view: u32) -> Result<Value> {
    let (root, rest) = path.split_once('\\').unwrap_or((path, ""));
    let root = if root.eq_ignore_ascii_case("HKLM") { HKEY_LOCAL_MACHINE } else if root.eq_ignore_ascii_case("HKU") { HKEY_USERS } else { return Err("仅支持 HKLM/HKU。".into()); };
    let access = match view { 32 => KEY_WOW64_32KEY, 64 => KEY_WOW64_64KEY, _ => return Err("注册表视图只能选 32 位或 64 位。".into()) };
    let key = reg_open_view(root, rest, false, access)?;
    let mut subkeys = Vec::new(); let mut values = Vec::new();
    unsafe {
        for i in 0..4096 {
            let mut name = vec![0; 32768]; let mut len = name.len() as u32;
            let status = RegEnumKeyExW(key.0, i, name.as_mut_ptr(), &mut len, null(), null_mut(), null_mut(), null_mut());
            if status == ERROR_NO_MORE_ITEMS { break; }
            if status != 0 { return Err(format!("枚举注册表子项失败：{status}")); }
            subkeys.push(String::from_utf16_lossy(&name[..len as usize]));
        }
        for i in 0..4096 {
            let mut name = vec![0; 32768]; let mut len = name.len() as u32; let mut kind = 0;
            let status = RegEnumValueW(key.0, i, name.as_mut_ptr(), &mut len, null(), &mut kind, null_mut(), null_mut());
            if status == ERROR_NO_MORE_ITEMS { break; }
            if status != 0 { return Err(format!("枚举注册表值失败：{status}")); }
            let name = String::from_utf16_lossy(&name[..len as usize]);
            values.push(json!({"name":name,"type":kind,"data":read_reg_value(key.0, &name).unwrap_or_else(|e|e)}));
        }
    }
    Ok(json!({"path":path,"view":view,"subkeys":subkeys,"values":values}))
}
pub fn account_sid(account: &str) -> Result<String> {
    let account=if let Some(user)=account.strip_prefix(".\\"){
        use windows_sys::Win32::System::SystemInformation::*;
        let mut name=[0u16;256];let mut size=name.len() as u32;
        unsafe{check(GetComputerNameExW(ComputerNameNetBIOS,name.as_mut_ptr(),&mut size),"读取目标机本地名称失败")?;}
        format!("{}\\{user}",String::from_utf16_lossy(&name[..size as usize]))
    }else{account.into()};
    unsafe {
        let mut size = 0; let mut domain_size = 0; let mut use_type = 0;
        LookupAccountNameW(null(), wide(&account).as_ptr(), null_mut(), &mut size, null_mut(), &mut domain_size, &mut use_type);
        if size == 0 { return Err(error("无法解析账号 SID")); }
        let mut sid = vec![0u8; size as usize]; let mut domain = vec![0; domain_size as usize];
        check(LookupAccountNameW(null(), wide(&account).as_ptr(), sid.as_mut_ptr().cast(), &mut size, domain.as_mut_ptr(), &mut domain_size, &mut use_type), "解析账号 SID 失败")?;
        let mut text = null_mut();
        check(windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW(sid.as_mut_ptr().cast(), &mut text), "转换 SID 失败")?;
        let result = string(text); LocalFree(text.cast()); Ok(result)
    }
}
pub fn transparent(account: &str, new_value: Option<u32>) -> Result<Value> {
    let sid = account_sid(account)?;
    let hive=user_hive(&sid,new_value.is_some())?;
    let path = "Software\\Policies\\Microsoft\\Windows\\Safer\\CodeIdentifiers";
    if let Some(value) = new_value {
        let mut key = null_mut();
        unsafe {
            let status = RegCreateKeyExW(hive.0, wide(path).as_ptr(), 0, null(), 0, KEY_SET_VALUE | KEY_WOW64_64KEY, null(), &mut key, null_mut());
            if status != 0 { return Err(format!("创建目标配置项失败：{status}")); }
            let key = Reg(key);
            let status = RegSetValueExW(key.0, wide("TransparentEnabled").as_ptr(), 0, REG_DWORD, value.to_le_bytes().as_ptr(), 4);
            if status != 0 { return Err(format!("写入失败：{status}；请核对目标机实际值。")); }
            let status=RegFlushKey(key.0);
            if status!=0{return Err(format!("写入后刷新用户配置失败：{status}，请核对实际值。"));}
        }
    }
    let mut current=0u32;let mut size=4;
    let status=unsafe{RegGetValueW(hive.0,wide(path).as_ptr(),wide("TransparentEnabled").as_ptr(),RRF_RT_REG_DWORD|RRF_SUBKEY_WOW6464KEY,null_mut(),(&mut current as *mut u32).cast(),&mut size)};
    let current=match status{ERROR_SUCCESS=>Some(current),ERROR_FILE_NOT_FOUND=>None,_=>return Err(format!("读取 TransparentEnabled 失败：{status}。未将权限或类型错误当作未设置。"))};
    if new_value.is_some()&&current!=new_value{return Err("已写入但回读验证失败，请核对目标机。".into());}
    Ok(json!({"account":account,"sid":sid,"key":format!("HKU\\{sid}\\{path}"),"value":"TransparentEnabled","data":current.map(|value|value.to_string()).unwrap_or_else(||"（未设置）".into()),"written":new_value.is_some()}))
}

pub fn expand_environment(value:&str)->Result<String>{
    use windows_sys::Win32::System::Environment::ExpandEnvironmentStringsW;
    unsafe{
        let value=wide(value);let needed=ExpandEnvironmentStringsW(value.as_ptr(),null_mut(),0);
        if needed==0||needed>32768{return Err(error("展开路径环境变量失败"));}
        let mut output=vec![0u16;needed as usize];
        let written=ExpandEnvironmentStringsW(value.as_ptr(),output.as_mut_ptr(),needed);
        if written==0||written>needed{return Err(error("展开路径环境变量失败"));}
        Ok(String::from_utf16_lossy(&output[..written.saturating_sub(1) as usize]))
    }
}
fn expand_variables(value:&str,variables:&std::collections::HashMap<String,String>)->Result<String>{
    let mut value=value.to_owned();
    for _ in 0..5{
        let mut expanded=String::new();let mut rest=value.as_str();
        while let Some(start)=rest.find('%'){
            expanded.push_str(&rest[..start]);let Some(end)=rest[start+1..].find('%').map(|offset|start+1+offset)else{expanded.push_str(&rest[start..]);rest="";break;};
            let name=&rest[start+1..end];expanded.push_str(variables.get(&name.to_lowercase()).map(String::as_str).unwrap_or(&rest[start..=end]));rest=&rest[end+1..];
        }
        expanded.push_str(rest);if expanded==value{break;}value=expanded;
    }
    if value.contains('%'){return Err(format!("目标用户目录存在未解析的环境变量：{value}"));}
    crate::model::drive_path(&value)?;Ok(value)
}
pub fn profile_folder(account:&str,name:&str,fallback:&str)->Result<std::path::PathBuf>{
    use std::collections::HashMap;
    let sid=account_sid(account)?;
    let profile=expand_environment(&reg_string(HKEY_LOCAL_MACHINE,&format!("SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion\\ProfileList\\{sid}"),"ProfileImagePath")?)?;
    crate::model::drive_path(&profile)?;
    let mut variables=HashMap::from([
        ("userprofile".into(),profile.clone()),("appdata".into(),format!("{profile}\\AppData\\Roaming")),("localappdata".into(),format!("{profile}\\AppData\\Local")),
        ("homedrive".into(),profile[..2].into()),("homepath".into(),profile[2..].into())
    ]);
    for key in ["Environment","Volatile Environment"]{
        if let Ok(data)=registry_browse(&format!("HKU\\{sid}\\{key}"),64){
            for value in data["values"].as_array().into_iter().flatten(){if let (Some(name),Some(data))=(value["name"].as_str(),value["data"].as_str()){variables.entry(name.to_lowercase()).or_insert_with(||data.into());}}
        }
    }
    for (name,value) in std::env::vars(){variables.entry(name.to_lowercase()).or_insert(value);}
    let value=reg_string(HKEY_USERS,&format!("{sid}\\Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\User Shell Folders"),name).unwrap_or_else(|_|format!("{profile}\\{fallback}"));
    Ok(std::path::PathBuf::from(expand_variables(&value,&variables)?))
}
pub fn common_folder(id:&windows_sys::core::GUID)->Result<std::path::PathBuf>{unsafe{
    let mut pointer=null_mut();let code=windows_sys::Win32::UI::Shell::SHGetKnownFolderPath(id,0,null_mut(),&mut pointer);
    if code<0{return Err(format!("读取系统公共目录失败：HRESULT {code:#x}"));}
    let path=string(pointer);
    #[link(name="ole32")]unsafe extern "system"{fn CoTaskMemFree(pointer:*mut std::ffi::c_void);}
    CoTaskMemFree(pointer.cast());crate::model::drive_path(&path)?;Ok(path.into())
}}
#[cfg(test)]mod profile_tests{use super::*;#[test]fn nested_user_environment_expansion_is_case_insensitive_and_rejects_unknown(){
    let variables=std::collections::HashMap::from([("userprofile".into(),"D:\\Users\\Target".into()),("onedrive".into(),"%USERPROFILE%\\OneDrive".into())]);
    assert_eq!(expand_variables("%OneDrive%\\Desktop",&variables).unwrap(),"D:\\Users\\Target\\OneDrive\\Desktop");
    assert!(expand_variables("%MISSING%\\Desktop",&variables).is_err());
}}
fn user_hive(sid:&str,write:bool)->Result<Reg>{
    let access=KEY_READ|if write{KEY_WRITE}else{0};
    let loaded=||{
        let mut key=null_mut();let status=unsafe{RegOpenKeyExW(HKEY_USERS,wide(sid).as_ptr(),0,access,&mut key)};
        if status==ERROR_SUCCESS{Ok(Reg(key))}else{Err(status)}
    };
    match loaded(){Ok(key)=>return Ok(key),Err(ERROR_FILE_NOT_FOUND)=>(),Err(error)=>return Err(format!("打开目标用户配置失败：{error}"))}
    let profile=reg_string(HKEY_LOCAL_MACHINE,&format!("SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion\\ProfileList\\{sid}"),"ProfileImagePath")
        .map_err(|_|"目标账号还没有本地用户配置。请先让该账号在目标电脑登录一次。".to_string())?;
    let file=std::path::PathBuf::from(expand_environment(&profile)?).join("NTUSER.DAT");
    for privilege in ["SeBackupPrivilege","SeRestorePrivilege"]{crate::admin::privilege(privilege)?;}
    let mut key=null_mut();
    let status=unsafe{RegLoadAppKeyW(wide(&file.to_string_lossy()).as_ptr(),&mut key,access,REG_PROCESS_APPKEY,0)};
    if status!=ERROR_SUCCESS{
        if let Ok(key)=loaded(){return Ok(key);}
        return Err(format!("无法加载目标账号的 NTUSER.DAT：{status}。未创建空的 HKU 项。"));
    }
    Ok(Reg(key))
}

pub fn restrict_directory(path: &std::path::Path) -> Result<()> {
    use windows_sys::Win32::Security::Authorization::*;
    unsafe {
        let mut descriptor = null_mut();
        check(ConvertStringSecurityDescriptorToSecurityDescriptorW(wide("D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)").as_ptr(), 1, &mut descriptor, null_mut()), "无法创建目录权限")?;
        let mut dacl = null_mut(); let mut present = 0; let mut defaulted = 0;
        let result = (|| {
            check(GetSecurityDescriptorDacl(descriptor, &mut present, &mut dacl, &mut defaulted), "无法取得目录权限")?;
            let status = SetNamedSecurityInfoW(wide(&path.to_string_lossy()).as_ptr(), SE_FILE_OBJECT, DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION, null_mut(), null_mut(), dacl, null());
            if status != 0 { return Err(format!("无法保护任务目录：{status}")); } Ok(())
        })();
        LocalFree(descriptor); result
    }
}

pub fn current_exe() -> Result<std::path::PathBuf> { std::env::current_exe().map_err(|e|e.to_string()) }
pub fn io<T>(result: std::io::Result<T>) -> Result<T> { result.map_err(|e|e.to_string()) }
pub fn set_session(token: HANDLE, session: u32) -> Result<()> {
    unsafe { check(SetTokenInformation(token, TokenSessionId, (&session as *const u32).cast::<c_void>(), 4), "设置交互会话失败") }
}



pub unsafe extern "system" fn listview_scroll(window:HWND,message:u32,w:WPARAM,l:LPARAM,_id:usize,_data:usize)->LRESULT{unsafe{
    use std::mem::{size_of,zeroed};
    use windows_sys::Win32::UI::{Controls::*,WindowsAndMessaging::*};
    if message==WM_HSCROLL&&matches!((w&0xffff) as i32,SB_THUMBTRACK|SB_THUMBPOSITION){
        let mut info=SCROLLINFO{cbSize:size_of::<SCROLLINFO>() as u32,fMask:SIF_ALL,..zeroed()};
        if GetScrollInfo(window,SB_HORZ,&mut info)!=0{
            let delta=info.nTrackPos-info.nPos;
            if delta!=0{SendMessageW(window,LVM_SCROLL,delta as usize,0);}
            return 0;
        }
    }
    windows_sys::Win32::UI::Shell::DefSubclassProc(window,message,w,l)
}}

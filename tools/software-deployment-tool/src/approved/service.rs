use super::{identity,pipe::Stream,policy::{self,Rule},store};
use crate::{model::{Credentials,Result},win::{self,Handle,wide}};
use serde::{Deserialize,Serialize};
use serde_json::{Value,json};
use std::{collections::{HashMap,VecDeque},mem::{size_of,zeroed},ptr::{null,null_mut},sync::{Mutex,OnceLock,atomic::{AtomicBool,AtomicUsize,Ordering}},time::{Duration,Instant}};
use windows_sys::Win32::{Foundation::*,Security::{*,Authorization::*},Storage::FileSystem::*,System::{IO::*,Pipes::*,Services::*,Threading::*}};
use zeroize::{Zeroize,Zeroizing};

#[derive(Default,Deserialize,Serialize)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
pub struct Request{
    pub operation:String,
    #[serde(default,skip_serializing_if="Option::is_none")]pub rule_id:Option<String>,
    #[serde(default,skip_serializing_if="Option::is_none")]pub rule:Option<Rule>,
    #[serde(default,skip_serializing_if="Option::is_none")]pub account:Option<String>,
    #[serde(default,skip_serializing_if="Option::is_none")]pub password:Option<String>,
}
impl Drop for Request{fn drop(&mut self){if let Some(password)=&mut self.password{password.zeroize();}}}
pub fn parse(bytes:&[u8])->Result<Request>{
    if bytes.len()>super::MAX_FRAME{return Err("PROTOCOL_TOO_LARGE".into());}
    let request:Request=serde_json::from_slice(bytes).map_err(|_|"PROTOCOL_INVALID".to_string())?;
    if !["list","launch","save","revoke","configure","logs"].contains(&request.operation.as_str()){return Err("PROTOCOL_INVALID".into());}
    if request.operation=="launch" {
        if request.rule.is_some()||request.account.is_some()||request.password.is_some(){return Err("PROTOCOL_INVALID: 启动只接受规则 UUID。".into());}
        policy::id(request.rule_id.as_deref().ok_or("PROTOCOL_INVALID")?)?;
    }
    Ok(request)
}

static STOP:AtomicBool=AtomicBool::new(false);
static STATUS:AtomicUsize=AtomicUsize::new(0);
static ACTIVE_PIPE:Mutex<usize>=Mutex::new(0);
static QUOTA:OnceLock<Mutex<HashMap<String,u32>>>=OnceLock::new();
struct Reservation(String);
impl Reservation{
    fn new(sid:&str)->Result<Self>{
        let mut quotas=QUOTA.get_or_init(||Mutex::new(HashMap::new())).lock().map_err(|_|"LAUNCH_LIMIT")?;
        if STOP.load(Ordering::Relaxed){return Err("SERVICE_STOPPING: 后台正在更新，请稍后重试。".into());}
        if quotas.values().sum::<u32>()>=8||quotas.get(sid).copied().unwrap_or(0)>=3{return Err("LAUNCH_LIMIT: 已达到并发限制。".into());}
        *quotas.entry(sid.into()).or_default()+=1;Ok(Self(sid.into()))
    }
}
impl Drop for Reservation{fn drop(&mut self){
    if let Some(quotas)=QUOTA.get(){if let Ok(mut quotas)=quotas.lock(){if let Some(count)=quotas.get_mut(&self.0){*count-=1;if *count==0{quotas.remove(&self.0);}}}}
}}
fn report(state:u32){
    let value=SERVICE_STATUS{dwServiceType:SERVICE_WIN32_OWN_PROCESS,dwCurrentState:state,
        dwControlsAccepted:if state==SERVICE_RUNNING{SERVICE_ACCEPT_STOP|SERVICE_ACCEPT_SHUTDOWN}else{0},..Default::default()};
    unsafe{SetServiceStatus(STATUS.load(Ordering::Relaxed) as SERVICE_STATUS_HANDLE,&value);}
}
pub fn run()->Result<()>{
    let mut token=null_mut();unsafe{win::check(OpenProcessToken(GetCurrentProcess(),TOKEN_QUERY,&mut token),"SERVICE_IDENTITY_INVALID")?;}
    let token=Handle(token);
    if identity::user_sid(token.0)?!="S-1-5-18"{return Err("SERVICE_IDENTITY_INVALID: 后台必须作为 SYSTEM 启动。".into());}
    let mut name=wide(super::SERVICE);
    let table=[SERVICE_TABLE_ENTRYW{lpServiceName:name.as_mut_ptr(),lpServiceProc:Some(service_main)},SERVICE_TABLE_ENTRYW::default()];
    unsafe{win::check(StartServiceCtrlDispatcherW(table.as_ptr()),"SERVICE_START_FAILED")}
}
unsafe extern "system" fn control(code:u32,_event:u32,_data:*mut std::ffi::c_void,_context:*mut std::ffi::c_void)->u32{
    if code==SERVICE_CONTROL_STOP||code==SERVICE_CONTROL_SHUTDOWN{
        {
            let Ok(quotas)=QUOTA.get_or_init(||Mutex::new(HashMap::new())).lock()else{return ERROR_SERVICE_CANNOT_ACCEPT_CTRL;};
            if quotas.values().any(|count|*count>0){return ERROR_SERVICE_CANNOT_ACCEPT_CTRL;}
            STOP.store(true,Ordering::Relaxed);
        }
        report(SERVICE_STOP_PENDING);
        if let Ok(handle)=ACTIVE_PIPE.lock(){if *handle!=0{unsafe{CancelIoEx(*handle as HANDLE,null());}}}
    }
    0
}
unsafe extern "system" fn service_main(_argc:u32,_argv:*mut *mut u16){
    let handle=unsafe{RegisterServiceCtrlHandlerExW(wide(super::SERVICE).as_ptr(),Some(control),null())};
    if handle.is_null(){return;}STATUS.store(handle as usize,Ordering::Relaxed);
    report(SERVICE_START_PENDING);
    let result=store::initialize().and_then(|_|{report(SERVICE_RUNNING);listen()});
    if let Err(error)=result{let _=store::audit("S-1-5-18","service","",&error,None);}
    report(SERVICE_STOPPED);
}
fn listen()->Result<()>{
    let mut rates:HashMap<String,VecDeque<Instant>>=HashMap::new();
    while !STOP.load(Ordering::Relaxed){
        let mut descriptor=null_mut();
        unsafe{win::check(ConvertStringSecurityDescriptorToSecurityDescriptorW(wide("D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;0x0012019B;;;AU)").as_ptr(),1,&mut descriptor,null_mut()),"PIPE_FAILED")?;}
        let attributes=SECURITY_ATTRIBUTES{nLength:size_of::<SECURITY_ATTRIBUTES>() as u32,lpSecurityDescriptor:descriptor,bInheritHandle:0};
        let handle=unsafe{CreateNamedPipeW(wide(&format!("\\\\.\\pipe\\{}",super::PIPE)).as_ptr(),PIPE_ACCESS_DUPLEX|FILE_FLAG_OVERLAPPED|FILE_FLAG_FIRST_PIPE_INSTANCE,
            PIPE_TYPE_BYTE|PIPE_READMODE_BYTE|PIPE_REJECT_REMOTE_CLIENTS,1,super::MAX_FRAME as u32,super::MAX_FRAME as u32,0,&attributes)};
        unsafe{LocalFree(descriptor);}
        if handle==INVALID_HANDLE_VALUE{return Err(win::error("PIPE_FAILED"));}
        let mut stream=Stream{handle:Handle(handle)};
        *ACTIVE_PIPE.lock().map_err(|_|"PIPE_FAILED")?=handle as usize;
        let event=Handle(unsafe{CreateEventW(null(),1,0,null())});
        if event.0.is_null(){return Err(win::error("PIPE_FAILED"));}
        let mut overlapped:OVERLAPPED=unsafe{zeroed()};overlapped.hEvent=event.0;
        let connected=unsafe{ConnectNamedPipe(handle,&mut overlapped)};
        let error=unsafe{GetLastError()};
        let ready=connected!=0||error==ERROR_PIPE_CONNECTED||
            (error==ERROR_IO_PENDING&&unsafe{WaitForSingleObject(event.0,INFINITE)}==WAIT_OBJECT_0);
        if ready&&!STOP.load(Ordering::Relaxed){
            let mut caller_sid="unknown".to_owned();let mut operation="invalid".to_owned();let mut rule_id=String::new();
            let response=(||{
                let bytes=Zeroizing::new(stream.read_frame()?);
                let request=parse(&bytes)?;operation=request.operation.clone();rule_id=request.rule_id.clone().unwrap_or_default();
                let (sid,admin,session)=caller(handle)?;caller_sid=sid.clone();
                let now=Instant::now();
                rates.retain(|_,history|history.back().is_some_and(|last|now.duration_since(*last)<Duration::from_secs(60)));
                if rates.len()>=1000&&!rates.contains_key(&sid){return Err("RATE_LIMITED".into());}
                let history=rates.entry(sid.clone()).or_default();
                while history.front().is_some_and(|old|now.duration_since(*old)>Duration::from_secs(60)){history.pop_front();}
                if history.len()>=30{return Err("RATE_LIMITED".into());}history.push_back(now);
                handle_request(&request,&sid,admin,session)
            })();
            let mut response=match response{Ok(value)=>value,Err(error)=>json!({"success":false,"code":error.split(':').next().unwrap_or("OPERATION_FAILED"),"message":error})};
            if store::audit(&caller_sid,&operation,&rule_id,response["code"].as_str().unwrap_or("OPERATION_FAILED"),response["pid"].as_u64().map(|pid|pid as u32)).is_err(){
                response=json!({"success":false,"code":"AUDIT_FAILED","message":"无法写入审计，操作结果需核对。"});
            }
            let _=stream.write_frame(response.to_string().as_bytes());
            let mut receipt=[0;1];let _=std::io::Read::read_exact(&mut stream,&mut receipt);
        }
        {
            let mut active=ACTIVE_PIPE.lock().map_err(|_|"PIPE_FAILED")?;
            *active=0;
            unsafe{DisconnectNamedPipe(handle);}
            drop(stream);
        }
    }
    Ok(())
}
fn caller(pipe:HANDLE)->Result<(String,bool,u32)>{
    struct Revert;
    impl Drop for Revert{fn drop(&mut self){unsafe{RevertToSelf();}}}
    unsafe{
        win::check(ImpersonateNamedPipeClient(pipe),"CALLER_INVALID")?;let _revert=Revert;
        let mut token=null_mut();win::check(OpenThreadToken(GetCurrentThread(),TOKEN_QUERY|TOKEN_DUPLICATE,1,&mut token),"CALLER_INVALID")?;
        let token=Handle(token);
        let level=identity::token_info(token.0,TokenImpersonationLevel)?;
        if *(level.as_ptr() as *const u32)<SecurityImpersonation as u32{return Err("CALLER_INVALID: 缺少身份模拟权限。".into());}
        let sid=identity::user_sid(token.0)?;let admin=identity::is_admin(token.0)?;
        let session=identity::token_info(token.0,TokenSessionId)?;
        let session=*(session.as_ptr() as *const u32);let mut pipe_session=0;
        win::check(GetNamedPipeClientSessionId(pipe,&mut pipe_session),"SESSION_INVALID")?;
        if pipe_session!=session{return Err("SESSION_INVALID".into());}
        Ok((sid,admin,session))
    }
}
fn handle_request(request:&Request,sid:&str,admin:bool,session:u32)->Result<Value>{
    if !matches!(request.operation.as_str(),"list"|"launch")&&!admin{return Err("ADMIN_REQUIRED".into());}
    match request.operation.as_str(){
        "list"=>{
            let rules:Vec<_>=store::read()?.into_iter().filter(|rule|admin||rule.allowed_sids.iter().any(|allowed|allowed==sid)).collect();
            let account=if admin{store::credentials().ok().map(|credential|credential.account.clone())}else{None};
            let active=QUOTA.get_or_init(||Mutex::new(HashMap::new())).lock().map_err(|_|"LAUNCH_LIMIT")?;
            let active=if admin{active.values().sum::<u32>()}else{active.get(sid).copied().unwrap_or_default()};
            Ok(json!({"success":true,"code":"OK","rules":rules,"account":account,"activeLaunches":active,"backendRevision":super::BACKEND_REVISION}))
        },
        "save"=>{let rule=request.rule.clone().ok_or("INVALID_RULE")?;let rule=store::save(rule)?;Ok(json!({"success":true,"code":"RULE_SAVED","rules":[rule]}))},
        "revoke"=>{store::revoke(request.rule_id.as_deref().ok_or("INVALID_RULE")?)?;Ok(json!({"success":true,"code":"RULE_REVOKED"}))},
        "configure"=>{
            let credentials=Credentials{account:request.account.clone().ok_or("INVALID_ACCOUNT")?,password:request.password.clone().ok_or("INVALID_ACCOUNT")?};
            store::configure(&credentials)?;Ok(json!({"success":true,"code":"ACCOUNT_CONFIGURED"}))
        },
        "logs"=>Ok(json!({"success":true,"code":"OK","message":store::logs()?})),
        "launch"=>{
            let id=policy::id(request.rule_id.as_deref().ok_or("INVALID_RULE")?)?;
            let rule=store::read()?.into_iter().find(|rule|policy::id(&rule.id).ok().as_ref()==Some(&id)).ok_or("RULE_NOT_FOUND")?;
            if !rule.authorize(sid,session){return Err("ACCESS_DENIED".into());}
            store::audit(sid,"launch-request",&rule.id,"AUTHORIZED",None)?;
            let reservation=Reservation::new(sid)?;
            let launched=identity::launch(&rule,session)?;let pid=launched.pid;
            let sid=sid.to_owned();
            std::thread::spawn(move||{
                let outcome=launched.wait();
                let code=outcome.map(|code|format!("PROCESS_EXIT_{code}")).unwrap_or("PROCESS_CLEANUP_FAILED".into());
                let _=store::audit(&sid,"process-exit",&rule.id,&code,Some(pid));drop(reservation);
            });
            Ok(json!({"success":true,"code":"PROCESS_CREATED","pid":pid}))
        },
        _=>Err("PROTOCOL_INVALID".into())
    }
}

pub fn send(request:&Request)->Result<Value>{
    unsafe{
        let pipe = wide(&format!("\\\\.\\pipe\\{}", super::PIPE));
        let deadline = Instant::now() + Duration::from_secs(3);
        let handle = loop {
            let handle = CreateFileW(pipe.as_ptr(), GENERIC_READ | GENERIC_WRITE, 0, null(), OPEN_EXISTING,
                FILE_FLAG_OVERLAPPED | SECURITY_SQOS_PRESENT | SECURITY_IMPERSONATION, null_mut());
            if handle != INVALID_HANDLE_VALUE { break handle; }
            let error = GetLastError();
            // SCM may report Running before the listener is ready, and the
            // single pipe instance is recreated after each request.
            if !matches!(error, ERROR_FILE_NOT_FOUND | ERROR_PIPE_BUSY) || Instant::now() >= deadline {
                return Err(format!("无法连接授权后台（BROKER_UNAVAILABLE）：{}", std::io::Error::from_raw_os_error(error as i32)));
            }
            std::thread::sleep(Duration::from_millis(40));
        };
        let mut stream=Stream{handle:Handle(handle)};
        let mut pid=0;win::check(GetNamedPipeServerProcessId(handle,&mut pid),"BROKER_IDENTITY_FAILED")?;
        let scm=OpenSCManagerW(null(),null(),SC_MANAGER_CONNECT);
        if scm.is_null(){return Err(win::error("BROKER_IDENTITY_FAILED"));}
        let service=OpenServiceW(scm,wide(super::SERVICE).as_ptr(),SERVICE_QUERY_STATUS);
        if service.is_null(){CloseServiceHandle(scm);return Err(win::error("BROKER_IDENTITY_FAILED"));}
        let mut status:SERVICE_STATUS_PROCESS=zeroed();let mut bytes=0;
        let ok=QueryServiceStatusEx(service,SC_STATUS_PROCESS_INFO,(&mut status as *mut SERVICE_STATUS_PROCESS).cast(),size_of::<SERVICE_STATUS_PROCESS>() as u32,&mut bytes);
        CloseServiceHandle(service);CloseServiceHandle(scm);
        if ok==0||status.dwCurrentState!=SERVICE_RUNNING||status.dwProcessId!=pid{return Err("BROKER_IDENTITY_FAILED".into());}
        let bytes=Zeroizing::new(serde_json::to_vec(request).map_err(|e|e.to_string())?);
        stream.write_frame(&bytes)?;
        let response=stream.read_frame()?;
        let _=std::io::Write::write_all(&mut stream,&[6]);
        let value:Value=serde_json::from_slice(&response).map_err(|e|e.to_string())?;
        if value["success"]!=true{return Err(value["message"].as_str().or_else(||value["code"].as_str()).unwrap_or("OPERATION_FAILED").into());}
        Ok(value)
    }
}

#[cfg(test)]
mod tests{
    use super::*;
    #[test]fn launch_rejects_extra_fields(){
        let id=crate::model::new_id();
        assert!(parse(format!(r#"{{"operation":"launch","ruleId":"{id}"}}"#).as_bytes()).is_ok());
        assert!(parse(format!(r#"{{"operation":"launch","ruleId":"{id}","password":"x"}}"#).as_bytes()).is_err());
        assert!(parse(format!(r#"{{"operation":"launch","ruleId":"{id}","target":"C:\\x.exe"}}"#).as_bytes()).is_err());
    }
}

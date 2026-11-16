use crate::{admin, crypto, model::{self, Request, Result}, win::{self, wide}};
use serde_json::{Value, json};
use std::{fs, mem::{size_of, zeroed}, os::windows::io::FromRawHandle, path::{Path, PathBuf}, ptr::{null, null_mut}, sync::{OnceLock, atomic::{AtomicBool, AtomicUsize, Ordering}}, time::{Duration, Instant}};
use windows_sys::Win32::{Foundation::*, Security::{*, Authorization::*}, Storage::FileSystem::*, System::{Services::*, Pipes::*, Threading::*, SystemInformation::*, Registry::*}};

static OPTIONS: OnceLock<(String, PathBuf)> = OnceLock::new();
static STATUS: AtomicUsize = AtomicUsize::new(0);
static STOP: AtomicBool = AtomicBool::new(false);

pub fn expand(s: &str) -> String { s.replace("%SystemRoot%", &std::env::var("SystemRoot").unwrap_or("C:\\Windows".into())) }
pub(crate) fn read_request(manifest: &Path) -> Result<Request> {
    let request: Request = serde_json::from_slice(&win::io(fs::read(manifest))?).map_err(|e|e.to_string())?;
    if request.id.len()!=32 || !request.id.bytes().all(|x|x.is_ascii_hexdigit()) || manifest.parent().and_then(|p|p.file_name()).and_then(|p|p.to_str()) != Some(request.id.as_str()) {
        return Err("任务目录与 ID 不匹配。".into());
    }
    Ok(request)
}
pub(crate) fn write_result(path: &Path, id: &str, outcome: Result<Value>) -> Result<()> {
    let value = match outcome { Ok(data)=>json!({"id":id,"success":true,"data":data}), Err(error)=>json!({"id":id,"success":false,"error":error}) };
    let temporary = path.with_extension("json.new");
    win::io(fs::write(&temporary, serde_json::to_vec_pretty(&value).map_err(|e|e.to_string())?))?;
    win::io(fs::rename(temporary, path))
}

pub fn service(args: &[String]) -> Result<()> {
    let name = args.get(2).ok_or("缺少服务名")?.clone();
    if !name.starts_with("CorpRust_") || name.len()!=21 || !name[9..].bytes().all(|c|c.is_ascii_hexdigit()) { return Err("服务名无效。".into()); }
    let manifest = PathBuf::from(expand(args.get(3).ok_or("缺少任务路径")?));
    OPTIONS.set((name.clone(),manifest)).map_err(|_|"服务已启动")?;
    let mut name = wide(&name);
    let table = [SERVICE_TABLE_ENTRYW { lpServiceName:name.as_mut_ptr(), lpServiceProc:Some(service_main) }, SERVICE_TABLE_ENTRYW {lpServiceName:null_mut(),lpServiceProc:None}];
    unsafe { win::check(StartServiceCtrlDispatcherW(table.as_ptr()), "服务调度失败") }
}
fn status(state: u32) {
    unsafe {
        let status = SERVICE_STATUS { dwServiceType:SERVICE_WIN32_OWN_PROCESS, dwCurrentState:state,
            dwControlsAccepted:if state==SERVICE_RUNNING {SERVICE_ACCEPT_STOP|SERVICE_ACCEPT_SHUTDOWN} else {0}, dwWin32ExitCode:0,
            dwServiceSpecificExitCode:0, dwCheckPoint:0, dwWaitHint:if state==SERVICE_STOP_PENDING {10000} else {0} };
        SetServiceStatus(STATUS.load(Ordering::Relaxed) as SERVICE_STATUS_HANDLE,&status);
    }
}
unsafe extern "system" fn control(code:u32,_event:u32,_data:*mut std::ffi::c_void,_context:*mut std::ffi::c_void)->u32 {
    if code==SERVICE_CONTROL_STOP || code==SERVICE_CONTROL_SHUTDOWN {
        STOP.store(true,Ordering::Relaxed);
        if let Some((_,manifest))=OPTIONS.get() {let _=fs::write(manifest.parent().unwrap().join("cancel.signal"),b"stop");}
        status(SERVICE_STOP_PENDING);
    }
    0
}
unsafe extern "system" fn service_main(_argc:u32,_argv:*mut *mut u16) {
    let (name,manifest)=OPTIONS.get().unwrap();
    let handle=unsafe {RegisterServiceCtrlHandlerExW(wide(name).as_ptr(),Some(control),null())};
    if handle.is_null() {return;} STATUS.store(handle as usize,Ordering::Relaxed); status(SERVICE_RUNNING);
    match read_request(manifest) {
        Ok(request)=>{
            let outcome=execute(&request,manifest);
            let _=write_result(&manifest.parent().unwrap().join("result.json"),&request.id,outcome);
        },
        Err(error)=>{let _=fs::write(manifest.with_extension("error.txt"),error);}
    }
    status(SERVICE_STOPPED);
}
fn execute(request:&Request,manifest:&Path)->Result<Value> {
    match request.operation.as_str() {
        "inventory"=>crate::inventory::collect(&request.inventory),
        "registry-query"=>win::transparent(&request.account,None),
        "registry-set"=>win::transparent(&request.account,Some(model::dword(&request.value)?)),
        "registry-browse"=>win::registry_browse(&request.key, request.registry_view),
        "security-info"=>crate::logs::security_info(),
        "security-clear"=>crate::logs::clear_security(),
        "diagnostic-logs"=>crate::logs::diagnostic(&request.logs,manifest.parent().unwrap()),
        "bcu-scan"=>crate::bcu::scan(manifest.parent().unwrap(),request),
        "bcu-delete-junk"=>crate::bcu_junk::delete(request,manifest.parent().unwrap()),
        "resolve-folders"=>crate::distribution::resolve(request),
        operation if operation.starts_with("elevation-")=>{
            broker(&[String::new(),"--broker".into(),manifest.to_string_lossy().into_owned()])?;
            let value:Value=serde_json::from_slice(&win::io(fs::read(manifest.with_file_name("broker-result.json")))?).map_err(|e|e.to_string())?;
            if value["success"]==true{Ok(value["data"].clone())}else{Err(value["error"].as_str().unwrap_or("授权操作失败。").into())}
        },
        "install"|"run"|"bcu-uninstall"=>{
            let job=admin::Job::new()?;
            let mut interactive=request.clone();
            let interactive_manifest=if request.operation=="bcu-uninstall"{
                interactive.bcu.session_user_sid=admin::session_user_sid(request.session)?;
                let path=manifest.with_file_name("session-request.json");
                win::io(fs::write(&path,serde_json::to_vec(&interactive).map_err(|e|e.to_string())?))?;path
            }else{manifest.to_owned()};
            let broker=admin::system_broker(&interactive,&interactive_manifest)?;
            if let Err(e)=job.assign(broker.handle.0) {unsafe{TerminateProcess(broker.handle.0,1);}return Err(e);}
            broker.resume()?;
            let deadline=Instant::now()+Duration::from_secs(request.timeout as u64+35);
            loop {
                if unsafe{WaitForSingleObject(broker.handle.0,200)}==WAIT_OBJECT_0 {break;}
                if STOP.load(Ordering::Relaxed)||Instant::now()>deadline {
                    return Err("任务被停止或超时；已结束本次任务进程组，请核对安装结果。".into());
                }
            }
            let bytes=win::io(fs::read(manifest.parent().unwrap().join("broker-result.json")))?;
            let value:Value=serde_json::from_slice(&bytes).map_err(|e|e.to_string())?;
            if value["id"]!=request.id {return Err("安装回执 ID 不匹配。".into());}
            if value["success"]!=true {return Err(value["error"].as_str().unwrap_or("管理员执行失败").into());}
            job.detach()?;
            Ok(value["data"].clone())
        },
        _=>Err("远端操作不受支持。".into()),
    }
}

pub fn broker(args:&[String])->Result<()> {
    let manifest=PathBuf::from(expand(args.get(2).ok_or("缺少任务路径")?));
    let request=read_request(&manifest)?;
    let result=(||{
        let (private,public)=crypto::keypair()?;
        unsafe {
            let mut sd=null_mut();
            win::check(ConvertStringSecurityDescriptorToSecurityDescriptorW(wide("D:P(A;;GA;;;SY)(A;;GA;;;BA)").as_ptr(),1,&mut sd,null_mut()),"创建凭据管道权限失败")?;
            let sa=SECURITY_ATTRIBUTES {nLength:size_of::<SECURITY_ATTRIBUTES>() as u32,lpSecurityDescriptor:sd,bInheritHandle:0};
            let handle=CreateNamedPipeW(wide(&format!("\\\\.\\pipe\\CorpDeployRust_{}",request.id)).as_ptr(),PIPE_ACCESS_DUPLEX|FILE_FLAG_FIRST_PIPE_INSTANCE,
                PIPE_TYPE_BYTE|PIPE_READMODE_BYTE|PIPE_WAIT,1,16384,16384,30000,&sa);
            LocalFree(sd);
            if handle==INVALID_HANDLE_VALUE {return Err(win::error("创建凭据管道失败"));}
            let mut pipe=fs::File::from_raw_handle(handle);
            win::io(fs::write(manifest.parent().unwrap().join("public.der.new"),public))?;
            win::io(fs::rename(manifest.parent().unwrap().join("public.der.new"),manifest.parent().unwrap().join("public.der")))?;
            if ConnectNamedPipe(handle,null_mut())==0 && GetLastError()!=ERROR_PIPE_CONNECTED {return Err(win::error("等待凭据通道失败"));}
            let (credentials,key)=crypto::receive(&mut pipe,&private,&request.id)?;
            if credentials.account!=request.account {return Err("凭据账号与任务不匹配。".into());}
            if request.operation.starts_with("elevation-"){
                crypto::acknowledge(&mut pipe,&key,&request.id)?;
                crate::approved::management::execute(&request,&credentials)
            }else{
                admin::install(&request,&credentials,&manifest,||crypto::acknowledge(&mut pipe,&key,&request.id))
            }
        }
    })();
    write_result(&manifest.parent().unwrap().join("broker-result.json"),&request.id,result)
}

pub fn inventory()->Result<Value> {
    unsafe {
        let mut memory:MEMORYSTATUSEX=zeroed();memory.dwLength=size_of::<MEMORYSTATUSEX>() as u32;
        win::check(GlobalMemoryStatusEx(&mut memory),"读取内存信息失败")?;
        let key="SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion";
        let mut disks=Vec::new();
        for letter in b'C'..=b'Z' {
            let root=format!("{}:\\",letter as char);let mut available=0;let mut total=0;let mut free=0;
            if GetDiskFreeSpaceExW(wide(&root).as_ptr(),&mut available,&mut total,&mut free)!=0 {
                disks.push(json!({"drive":root,"totalBytes":total,"freeBytes":free}));
            }
        }
        Ok(json!({"computer":std::env::var("COMPUTERNAME").unwrap_or_default(),"os":win::reg_string(HKEY_LOCAL_MACHINE,key,"ProductName").unwrap_or_default(),
            "displayVersion":win::reg_string(HKEY_LOCAL_MACHINE,key,"DisplayVersion").unwrap_or_default(),"build":win::reg_string(HKEY_LOCAL_MACHINE,key,"CurrentBuildNumber").unwrap_or_default(),
            "cpu":win::reg_string(HKEY_LOCAL_MACHINE,"HARDWARE\\DESCRIPTION\\System\\CentralProcessor\\0","ProcessorNameString").unwrap_or_default(),
            "architecture":std::env::consts::ARCH,"memoryBytes":memory.ullTotalPhys,"availableMemoryBytes":memory.ullAvailPhys,"disks":disks,"sessions":win::sessions(None)?}))
    }
}

pub fn self_test()->Result<String> {
    let embedded_bcu = crate::bundle::Executable::current()?.verify()?;
    model::hosts("localhost,127.0.0.1")?;
    model::drive_path("C:\\CorpRustSelfTest")?;
    if model::drive_path("C:\\..\\Windows").is_ok()||model::dword("0xFFFFFFFF")?!=u32::MAX||model::dword("4294967296").is_ok(){return Err("路径或 DWORD 边界检查失败。".into());}
    let inventory=inventory()?;
    let registry_path = "HKLM\\SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion";
    let registry=win::registry_browse(registry_path, 64)?;
    if win::registry_browse(registry_path, 32)?["view"] != 32 || registry["view"] != 64 {
        return Err("注册表视图检查失败。".into());
    }
    let listener=win::io(std::net::TcpListener::bind("127.0.0.1:0"))?;
    let address=win::io(listener.local_addr())?;
    let server=std::thread::spawn(move||->Result<()> {
        let (mut socket,_)=win::io(listener.accept())?;
        let (private,public)=crypto::keypair()?;
        use std::io::Write;
        win::io(socket.write_all(&(public.len() as u32).to_le_bytes()))?;win::io(socket.write_all(&public))?;
        let (credentials,key)=crypto::receive(&mut socket,&private,"self-test")?;
        if credentials.password!="test-only" {return Err("协议回环失败。".into());}
        crypto::acknowledge(&mut socket,&key,"self-test")
    });
    let mut client=win::io(std::net::TcpStream::connect(address))?;
    use std::io::Read;
    let mut length=[0;4];win::io(client.read_exact(&mut length))?;
    let mut public=vec![0;u32::from_le_bytes(length) as usize];win::io(client.read_exact(&mut public))?;
    crypto::send(&mut client,&public,"self-test",&model::Credentials{account:"test".into(),password:"test-only".into()})?;
    server.join().map_err(|_|"协议测试线程异常")??;
    serde_json::to_string_pretty(&json!({"passed":true,"embeddedBcu":embedded_bcu,"tests":["host/path/dword validation","native local WTS","native local registry read","native memory/disk/OS inventory","RSA-OAEP/AES-GCM TCP loopback"],"inventory":inventory,"registryValueCount":registry["values"].as_array().map(|v|v.len())})).map_err(|e|e.to_string())
}

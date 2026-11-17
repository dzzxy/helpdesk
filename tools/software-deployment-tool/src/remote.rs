use crate::{crypto, model::{self, Credentials, Request, Result}, win::{self, NetworkIdentity, wide}};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{fs, io::{Read, Write}, path::{Path, PathBuf}, ptr::{null, null_mut}, sync::{Arc, atomic::{AtomicBool, Ordering}}, time::{Duration, Instant}};
use windows_sys::Win32::{Storage::FileSystem::DELETE, System::Services::*};

pub type Reporter = Arc<dyn Fn(String) + Send + Sync>;
struct ServiceHandle(SC_HANDLE);
impl Drop for ServiceHandle { fn drop(&mut self) { unsafe { CloseServiceHandle(self.0); } } }
struct Scope { scm: ServiceHandle, service: Option<ServiceHandle>, directory: PathBuf, remote: String, clean: bool }
impl Scope {
    fn new(computer: &str, request: &Request) -> Result<Self> {
        win::io(fs::read_dir(format!("\\\\{computer}\\ADMIN$"))).map_err(|error|format!("无法读取 ADMIN$ 管理共享：{error}。请核对管理员凭据、共享访问权限及 SMB 445；未创建服务。"))?;
        let scm = unsafe { OpenSCManagerW(wide(&format!("\\\\{computer}")).as_ptr(), null(), SC_MANAGER_CONNECT | SC_MANAGER_CREATE_SERVICE) };
        if scm.is_null() { return Err(win::error("ADMIN$ 已可访问，但服务管理器拒绝连接或创建服务；请核对管理员完整权限及 RPC 策略")); }
        let scm = ServiceHandle(scm);
        let directory = PathBuf::from(format!("\\\\{computer}\\ADMIN$\\CorpInstallerRustTemp\\{}", request.id));
        // Create only a fresh task directory. Never reuse or recursively remove another job.
        win::io(fs::create_dir_all(directory.parent().unwrap()))?;
        win::io(fs::create_dir(&directory))?;
        if let Err(e) = win::restrict_directory(&directory) { let _ = fs::remove_dir(&directory); return Err(e); }
        Ok(Self { scm, service: None, directory,
            remote: format!("%SystemRoot%\\CorpInstallerRustTemp\\{}", request.id), clean: true })
    }
    fn start(&mut self, request: &Request) -> Result<()> {
        let name = format!("CorpRust_{}", &request.id[..12]);
        let command = format!("\"{}\\CorpDeployRust.exe\" --service {name} \"{}\\request.json\"", self.remote, self.remote);
        unsafe {
            let handle = CreateServiceW(self.scm.0, wide(&name).as_ptr(), wide(&format!("Corp Rust test {}", &request.id[..12])).as_ptr(),
                SERVICE_START | SERVICE_STOP | SERVICE_QUERY_STATUS | DELETE, SERVICE_WIN32_OWN_PROCESS, SERVICE_DEMAND_START, SERVICE_ERROR_NORMAL,
                wide(&command).as_ptr(), null(), null_mut(), null(), null(), null());
            if handle.is_null() { return Err(win::error("创建一次性服务失败")); }
            self.service = Some(ServiceHandle(handle));
            win::check(StartServiceW(handle, 0, null()), "启动一次性服务失败")?;
        }
        Ok(())
    }
    fn stopped(&self) -> bool {
        let Some(service) = &self.service else { return true; };
        unsafe { let mut status: SERVICE_STATUS = std::mem::zeroed(); QueryServiceStatus(service.0, &mut status) != 0 && status.dwCurrentState == SERVICE_STOPPED }
    }
}
impl Drop for Scope {
    fn drop(&mut self) {
        if let Some(service) = &self.service {
            unsafe { if self.clean { let mut s = std::mem::zeroed(); ControlService(service.0, SERVICE_CONTROL_STOP, &mut s); } DeleteService(service.0); }
        }
        if self.clean {
            let start = Instant::now();
            while start.elapsed() < Duration::from_secs(5) {
                // The path is always task-owned, includes a validated random ID and is under the fixed root.
                if fs::remove_dir_all(&self.directory).is_ok() || !self.directory.exists() { break; }
                std::thread::sleep(Duration::from_millis(200));
            }
        }
    }
}

pub fn execute(computer: &str, credentials: &Credentials, request: &Request, report: &Reporter, cancelled: &AtomicBool) -> Result<Value> {
    execute_with_source(computer,credentials,None,request,report,cancelled,None)
}
pub fn execute_with_source(computer:&str,credentials:&Credentials,source_credentials:Option<&Credentials>,request:&Request,report:&Reporter,cancelled:&AtomicBool,progress:Option<&(dyn Fn(Value)+Sync)>)->Result<Value>{
    request.validate()?; model::host(computer)?;
    if request.operation=="connection-repair"{return crate::repair::execute(computer,credentials,request,report,cancelled);}
    let _identity = NetworkIdentity::new(credentials)?;
    if request.operation == "sessions" {
        report(format!("{computer}：通过 WTS 只读查询会话…"));
        return win::sessions(Some(computer));
    }
    if request.operation == "distribute" { return crate::distribution::run(computer,credentials,source_credentials,request,report,cancelled,progress); }
    let mut remote_request = request.clone();
    if request.operation.starts_with("elevation-") && matches!(request.operation.as_str(),"elevation-preview"|"elevation-discover"|"elevation-create"|"elevation-approve"){
        let sessions=win::sessions(Some(computer))?;
        let available:Vec<_>=sessions.as_array().into_iter().flatten().filter(|row|row["state"]=="Active"
            &&(request.session==0||row["sessionId"]==request.session)).collect();
        let user=match available.as_slice(){
            [user]=>user["account"].as_str().ok_or("无法读取目标用户。")?,
            _=>return Err("请检测并选择目标电脑的活动登录用户。".into())
        };
        remote_request.elevation.user_account=user.into();
    }
    if matches!(request.operation.as_str(), "install" | "run" | "bcu-uninstall") && request.session == 0 {
        let sessions = win::sessions(Some(computer))?;
        let active: Vec<_> = sessions.as_array().into_iter().flatten()
            .filter(|row| row["state"] == "Active")
            .filter_map(|row| {
                let id = u32::try_from(row["sessionId"].as_u64()?).ok()?;
                (id != 0).then_some((id, row))
            })
            .collect();
        match active.as_slice() {
            [(session, row)] => {
                remote_request.session = *session;
                if let Some(progress) = progress {
                    progress(json!({"interactiveSession": row}));
                }
            }
            [] => return Err("目标机没有活动登录会话。".into()),
            _ => return Err("目标机有多个活动会话，请单独检测这台电脑并选择会话。".into()),
        }
    }
    if cancelled.load(Ordering::Relaxed) { return Err("已取消。".into()); }
    report(format!("{computer}：连接 ADMIN$ / SCM，准备原生 Rust 一次性组件…"));
    let mut scope = Scope::new(computer, request)?;
    let mut executable = crate::bundle::Executable::current()?;
    copy_reader(executable.core()?, &scope.directory.join("CorpDeployRust.exe"), cancelled, |_| {})?;
    if matches!(request.operation.as_str(),"bcu-scan"|"bcu-uninstall"){
        report(format!("{computer}：校验并上传 BCU 引擎…"));
        for (index, (name, expected)) in crate::bundle::ENGINES.into_iter().enumerate() {
            let destination = scope.directory.join(name);
            let mut hash = Sha256::new();
            if let Some(asset) = executable.asset(index)? {
                copy_reader(asset, &destination, cancelled, |chunk| hash.update(chunk))?;
            } else {
                let path = crate::bcu::payload_directory()?.join(name);
                copy_file(&path, &destination, cancelled, |chunk| hash.update(chunk))?;
            }
            if format!("{:x}", hash.finalize()) != expected {
                return Err(format!("BCU 组件校验失败：{name}"));
            }
        }
    }
    if (request.operation == "install" || request.operation == "run") && !request.existing {
        let name = Path::new(&request.source).file_name()
            .and_then(|name| name.to_str()).ok_or("无法读取安装包原始文件名。")?;
        let package_dir = scope.directory.join("package");
        win::io(fs::create_dir(&package_dir))?;
        report(format!("{computer}：上传安装文件…"));
        let upload_start = Instant::now();
        let mut hash = Sha256::new();
        let size = copy_file(Path::new(&request.source), &package_dir.join(name), cancelled, |chunk| hash.update(chunk))?;
        remote_request.sha256 = format!("{:x}", hash.finalize());
        report(format!("{computer}：安装包上传并计算校验值完成，{:.1} MiB，耗时 {:.1} 秒。", size as f64 / 1048576.0, upload_start.elapsed().as_secs_f64()));
        remote_request.source = format!("{}\\package\\{name}", scope.remote);
        remote_request.existing = true;
    }
    let request_path = scope.directory.join("request.json");
    win::io(fs::write(&request_path, serde_json::to_vec(&remote_request).map_err(|e|e.to_string())?))?;
    if cancelled.load(Ordering::Relaxed) { return Err("已取消。".into()); }
    scope.start(request)?;
    let deadline = Instant::now() + Duration::from_secs(request.timeout as u64 + 45);
    let result_path = scope.directory.join("result.json");
    if request.operation == "install" || request.operation == "run" || request.operation.starts_with("elevation-") {
        let public_path = scope.directory.join("public.der");
        let public_deadline = Instant::now() + Duration::from_secs(40);
        while !public_path.exists() {
            if let Ok(bytes) = fs::read(&result_path) { return decode_result(&bytes, &request.id); }
            if Instant::now() > public_deadline || cancelled.load(Ordering::Relaxed) { return Err("等待管理员凭据通道超时或已取消。".into()); }
            std::thread::sleep(Duration::from_millis(150));
        }
        let public = win::io(fs::read(public_path))?;
        let pipe_path = format!("\\\\{computer}\\pipe\\CorpDeployRust_{}", request.id);
        report(format!("{computer}：通过加密通道启动目标会话中的管理员进程…"));
        let c = credentials.clone(); let id = request.id.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        // A native pipe call can block; keep it out of the task/UI thread with a bounded wait.
        std::thread::spawn(move || {
            let outcome = (|| {
                let _identity = NetworkIdentity::new(&c)?;
                let end = Instant::now() + Duration::from_secs(20);
                let mut pipe = loop {
                    match fs::OpenOptions::new().read(true).write(true).open(&pipe_path) {
                        Ok(p) => break p,
                        Err(e) if Instant::now() < end => { let _ = e; std::thread::sleep(Duration::from_millis(100)); },
                        Err(e) => return Err(e.to_string()),
                    }
                };
                crypto::send(&mut pipe, &public, &id, &c)
            })(); let _ = tx.send(outcome);
        });
        let credential_deadline = Instant::now() + Duration::from_secs(45);
        let reply = loop {
            match rx.recv_timeout(Duration::from_millis(150)) {
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) if Instant::now() < credential_deadline => {
                    if cancelled.load(Ordering::Relaxed) {
                        return Err("已取消，正在清理临时服务。".into());
                    }
                }
                reply => break reply,
            }
        };
        match reply {
            Ok(Ok(())) if request.operation == "install" => report(format!("{computer}：等待目标电脑用户在「IT 软件安装」窗口确认…")),
            Ok(Ok(())) => report(format!("{computer}：身份通道已建立，等待执行结果…")),
            outcome => {
                if let Ok(bytes) = fs::read(&result_path) { return decode_result(&bytes, &request.id); }
                return Err(format!("管理员启动未确认：{}", match outcome { Ok(Err(e))=>e, _=>"凭据通道超时".into() }));
            }
        }
    }
    let mut stopped_at = None;
    let mut cancellation_deadline = None;
    let mut confirmation_pending = request.operation == "install";
    let mut last_progress=Vec::new();
    loop {
        if let Ok(bytes)=fs::read(scope.directory.join("progress.json")){
            if bytes!=last_progress{
                if let Ok(data)=serde_json::from_slice::<Value>(&bytes){if data["id"]==request.id{
                    report(format!("{computer}：{}",data["detail"].as_str().unwrap_or_default()));
                    if let Some(progress)=progress{progress(data);}
                    last_progress=bytes;
                }}
            }
        }
        if confirmation_pending && scope.directory.join("install-confirmed.signal").exists() {
            confirmation_pending = false;
            report(format!("{computer}：用户已确认，正在启动安装器…"));
        }
        if let Ok(bytes) = fs::read(&result_path) {
            let mut result = decode_result(&bytes, &request.id);
            if request.operation == "diagnostic-logs" {
                if let Ok(data) = &mut result {
                    let directory=crate::tasks::data_directory()?.join("Diagnostics");
                    win::io(fs::create_dir_all(&directory))?;
                    let path=directory.join(format!("{computer}-{}.zip",request.id));
                    copy_file(&scope.directory.join("diagnostic.zip"),&path,cancelled,|_|{})?;
                    data["localBundle"]=json!(path);
                }
            }
            if let Ok(data) = &result {
                if request.operation == "bcu-scan" && data["applications"].is_array() {
                    if let Some(progress) = progress {
                        progress(json!({"scanReceipt": data}));
                    }
                }
                let state = match data["state"].as_str() {
                    Some("Succeeded") => "安装成功",
                    Some("RebootRequired") => "安装成功，需重启",
                    Some("Unconfirmed") => "安装结果待确认",
                    Some("Started") => "程序已启动",
                    Some("WindowShown")=>"程序已启动，检测到软件窗口",
                    Some("Exited") => "程序已退出",
                    _ => "已收到结果",
                };
                report(format!("{computer}：{state}，正在清理临时服务和文件。"));
            }
            return result;
        }
        if cancelled.load(Ordering::Relaxed) {
            let stop_at = cancellation_deadline.get_or_insert_with(|| {
                let _ = fs::write(scope.directory.join("cancel.signal"), b"cancel");
                Instant::now() + Duration::from_secs(5)
            });
            if Instant::now() >= *stop_at {
                return Err("取消未收到目标机回执，正在清理临时服务。".into());
            }
        }
        if scope.stopped() {
            let since = stopped_at.get_or_insert_with(Instant::now);
            if since.elapsed() > Duration::from_secs(5) { return Err("目标临时服务已停止，但没有收到有效结果；请核对目标机。".into()); }
        }
        if Instant::now() > deadline {
            let _ = fs::write(scope.directory.join("cancel.signal"), b"timeout");
            return Err("任务超时；已请求停止，请核对目标机实际状态。".into());
        }
        std::thread::sleep(Duration::from_millis(300));
    }
}

fn decode_result(bytes: &[u8], id: &str) -> Result<Value> {
    let v: Value = serde_json::from_slice(bytes).map_err(|e|e.to_string())?;
    if v["id"].as_str() != Some(id) { return Err("任务回执 ID 不匹配。".into()); }
    if v["success"] == true { Ok(v["data"].clone()) }
    else { Err(v["error"].as_str().unwrap_or("目标任务失败").into()) }
}

fn copy_file(source: &Path, destination: &Path, cancelled: &AtomicBool, on_chunk: impl FnMut(&[u8])) -> Result<u64> {
    copy_reader(win::io(fs::File::open(source))?, destination, cancelled, on_chunk)
}

fn copy_reader(mut input: impl Read, destination: &Path, cancelled: &AtomicBool, mut on_chunk: impl FnMut(&[u8])) -> Result<u64> {
    let mut output = win::io(fs::OpenOptions::new().create_new(true).write(true).open(destination))?;
    let mut bytes = vec![0; 1024 * 1024]; let mut total = 0;
    loop {
        if cancelled.load(Ordering::Relaxed) { drop(output); let _ = fs::remove_file(destination); return Err("已取消，当前未完成文件已删除。".into()); }
        let count = win::io(input.read(&mut bytes))?; if count == 0 { break; }
        win::io(output.write_all(&bytes[..count]))?; total += count as u64;
        on_chunk(&bytes[..count]);
    }
    win::io(output.sync_all())?; Ok(total)
}
#[cfg(test)]
fn distribute_tree(source: &Path, destination: &Path, policy: model::ConflictPolicy, cancelled: &AtomicBool) -> Result<Value> {
    let mut stats = CopyStats::default();
    #[derive(Default)]
    struct CopyStats { copied: u64, skipped: u64, bytes: u64 }
    fn copy_tree(src: &Path, dest: &Path, policy: model::ConflictPolicy, cancel: &AtomicBool, stats: &mut CopyStats) -> Result<()> {
        if cancel.load(Ordering::Relaxed) { return Err("已取消分发。".into()); }
        let metadata = win::io(fs::symlink_metadata(src))?;
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 { return Err(format!("不跟随符号链接或目录联接：{}", src.display())); }
        if metadata.is_dir() {
            if let Ok(current) = fs::symlink_metadata(dest) {
                if !current.is_dir() || current.file_attributes() & 0x400 != 0 {
                    return Err(format!("目标不是普通目录：{}", dest.display()));
                }
            }
            win::io(fs::create_dir_all(dest))?;
            for item in win::io(fs::read_dir(src))? { let item = win::io(item)?; copy_tree(&item.path(), &dest.join(item.file_name()), policy, cancel, stats)?; }
        } else {
            use model::ConflictPolicy::*;
            let overwrite = match fs::symlink_metadata(dest) {
                Ok(current) if current.is_file() && current.file_attributes() & 0x400 == 0 => {
                    let replace = match policy {
                        Stop | Ask => return Err(format!("目标文件已存在：{}", dest.display())),
                        Overwrite => true,
                        Skip => false,
                        Newer => win::io(metadata.modified())? > win::io(current.modified())?,
                        DifferentHash => metadata.len() != current.len() || hash_file(src)? != hash_file(dest)?,
                    };
                    if !replace { stats.skipped += 1; return Ok(()); }
                    true
                }
                Ok(_) => return Err(format!("目标不是普通文件：{}", dest.display())),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
                Err(error) => return Err(error.to_string()),
            };
            if let Some(parent) = dest.parent() { win::io(fs::create_dir_all(parent))?; }
            let temporary = dest.with_file_name(format!(".corprust-{}.tmp", model::new_id()));
            let copied = match copy_file(src, &temporary, cancel, |_| {}) { Ok(n)=>n, Err(e)=>{let _=fs::remove_file(&temporary);return Err(e);} };
            if cancel.load(Ordering::Relaxed) {
                let _ = fs::remove_file(&temporary);
                return Err("已取消分发。".into());
            }
            // Rust rename uses the Windows replace operation: old file remains until the new copy is complete.
            use windows_sys::Win32::Storage::FileSystem::{MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH};
            if unsafe { MoveFileExW(wide(&temporary.to_string_lossy()).as_ptr(), wide(&dest.to_string_lossy()).as_ptr(), MOVEFILE_WRITE_THROUGH | if overwrite { MOVEFILE_REPLACE_EXISTING } else {0}) } == 0 {
                let error=win::error("提交分发文件失败");let _=fs::remove_file(temporary);return Err(error);
            }
            stats.bytes += copied; stats.copied += 1;
        }
        Ok(())
    }
    copy_tree(source, destination, policy, cancelled, &mut stats)?;
    Ok(json!({"files":stats.copied,"skipped":stats.skipped,"bytes":stats.bytes,"destination":destination}))
}

#[cfg(test)]
#[test]
fn distribution_conflicts_preserve_or_replace_content() {
    use model::ConflictPolicy::*;
    let directory = std::env::temp_dir().join(model::new_id());
    fs::create_dir(&directory).unwrap();
    let source = directory.join("source.txt");
    let destination = directory.join("destination.txt");
    let cancelled = AtomicBool::new(false);
    fs::write(&source, b"new").unwrap();
    fs::write(&destination, b"old").unwrap();
    assert!(distribute_tree(&source, &destination, Stop, &cancelled).is_err());
    assert_eq!(distribute_tree(&source, &destination, Skip, &cancelled).unwrap()["skipped"], 1);
    assert_eq!(fs::read(&destination).unwrap(), b"old");
    assert_eq!(distribute_tree(&source, &destination, DifferentHash, &cancelled).unwrap()["files"], 1);
    assert_eq!(fs::read(&destination).unwrap(), b"new");
    assert_eq!(distribute_tree(&source, &destination, DifferentHash, &cancelled).unwrap()["skipped"], 1);
    cancelled.store(true, Ordering::Relaxed);
    assert!(distribute_tree(&source, &destination, Overwrite, &cancelled).is_err());
    fs::remove_file(source).unwrap();
    fs::remove_file(destination).unwrap();
    fs::remove_dir(directory).unwrap();
}

pub fn hash_file(path: &Path) -> Result<String> {
    use sha2::{Digest, Sha256};
    let mut file=win::io(fs::File::open(path))?;let mut hash=Sha256::new();let mut buffer=vec![0;1024*1024];
    loop {let count=win::io(file.read(&mut buffer))?;if count==0{break;}hash.update(&buffer[..count]);}
    Ok(format!("{:x}",hash.finalize()))
}


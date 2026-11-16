use crate::{model::{Credentials,Request,Result},win::{self,Handle,wide}};
use base64::{Engine,engine::general_purpose::STANDARD};
use serde_json::{Value,json};
use sha2::{Digest,Sha256};
use std::{fs,io::{Read,Write},os::windows::process::CommandExt,process::{Command,Stdio},ptr::{null,null_mut},sync::atomic::{AtomicBool,Ordering},time::{Duration,Instant}};
use windows_sys::Win32::{Foundation::*,Security::{*,Authorization::*},Storage::FileSystem::*,System::{IO::*,Pipes::*,Threading::*},UI::WindowsAndMessaging::*};
use zeroize::Zeroizing;

fn powershell()->String{
    format!("{}\\System32\\WindowsPowerShell\\v1.0\\powershell.exe",std::env::var("SystemRoot").unwrap_or("C:\\Windows".into()))
}
fn encoded(script:&str)->String{STANDARD.encode(script.encode_utf16().flat_map(u16::to_le_bytes).collect::<Vec<_>>())}
fn script(id:&str)->String{include_str!("repair.ps1").replace("__TASK_ID__",id)}
pub(crate) fn output(mut command:Command,stdin:Option<Zeroizing<Vec<u8>>>,timeout:Duration,cancel:&AtomicBool)->Result<String>{
    command.stdin(if stdin.is_some(){Stdio::piped()}else{Stdio::null()}).stdout(Stdio::piped()).stderr(Stdio::piped()).creation_flags(0x08000000);
    let mut child=win::io(command.spawn())?;
    if let Some(input)=stdin{win::io(child.stdin.take().ok_or("无法创建命令输入通道。")?.write_all(&input))?;}
    let mut stdout=child.stdout.take().ok_or("无法读取命令输出。")?;
    let mut stderr=child.stderr.take().ok_or("无法读取命令错误。")?;
    let out=std::thread::spawn(move||{let mut bytes=Vec::new();stdout.by_ref().take(256*1024).read_to_end(&mut bytes).map(|_|bytes)});
    let err=std::thread::spawn(move||{let mut bytes=Vec::new();stderr.by_ref().take(64*1024).read_to_end(&mut bytes).map(|_|bytes)});
    let end=Instant::now()+timeout;
    loop{
        if win::io(child.try_wait())?.is_some(){break;}
        if cancel.load(Ordering::Relaxed)||Instant::now()>end{
            let _=child.kill();let _=child.wait();
            return Err("远程调用取消或超时，目标可能已部分修改，请重新诊断。".into());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let stdout=win::io(out.join().map_err(|_|"输出线程异常。")?)?;
    let stderr=win::io(err.join().map_err(|_|"错误输出线程异常。")?)?;
    let value=String::from_utf8_lossy(&stdout).to_string();
    if value.trim().is_empty(){let error=String::from_utf8_lossy(&stderr).chars().take(4000).collect::<String>();return Err(if error.trim().is_empty(){"外部调用没有返回有效输出。".into()}else{error});}
    Ok(value)
}
fn trusted_ssh(host:&str,cancel:&AtomicBool)->Result<std::path::PathBuf>{
    let mut scan=Command::new("ssh-keyscan.exe");
    scan.args(["-T","5","-t","ed25519",host]);
    let keys=output(scan,None,Duration::from_secs(15),cancel)?;
    let line=keys.lines().find(|line|!line.starts_with('#')&&line.split_whitespace().count()==3).ok_or("目标机没有返回 SSH 主机密钥。")?;
    let fields:Vec<_>=line.split_whitespace().collect();
    let decoded=STANDARD.decode(fields[2]).map_err(|_|"SSH 主机密钥无效。")?;
    let fingerprint=STANDARD.encode(Sha256::digest(&decoded)).trim_end_matches('=').to_owned();
    let directory=crate::tasks::data_directory()?.join("SSH");win::io(fs::create_dir_all(&directory))?;
    let path=directory.join(format!("{host}.known_hosts"));
    match fs::read_to_string(&path){
        Ok(previous) if previous.split_whitespace().nth(2)!=Some(fields[2])=>return Err("SSH 主机指纹已变化，请先核对目标电脑。".into()),
        Ok(_)=>(),
        Err(error) if error.kind()==std::io::ErrorKind::NotFound=>{
            let text=format!("目标 {host}\n{}\nSHA256:{fingerprint}\n是否信任此主机？",fields[1]);
            if unsafe{MessageBoxW(null_mut(),wide(&text).as_ptr(),wide("确认 SSH 主机指纹").as_ptr(),MB_YESNO|MB_DEFBUTTON2|MB_ICONQUESTION)}!=IDYES{return Err("未信任 SSH 主机，未发送修复命令。".into());}
            if cancel.load(Ordering::Relaxed){return Err("已取消。".into());}
            win::io(fs::write(&path,line))?;
        },
        Err(error)=>return Err(error.to_string())
    }
    Ok(path)
}
fn askpass_server(password:Zeroizing<String>)->Result<(String,std::thread::JoinHandle<Result<()>>)>{
    let name=format!("CorpRustSshAskpass_{}",crate::model::new_id());
    let mut token=null_mut();
    unsafe{win::check(OpenProcessToken(GetCurrentProcess(),TOKEN_QUERY,&mut token),"读取当前身份失败")?;}
    let token=Handle(token);let sid=crate::approved::identity::user_sid(token.0)?;
    let mut descriptor=null_mut();
    unsafe{win::check(ConvertStringSecurityDescriptorToSecurityDescriptorW(wide(&format!("D:P(A;;GA;;;SY)(A;;GA;;;{sid})")).as_ptr(),1,&mut descriptor,null_mut()),"创建密码通道权限失败")?;}
    let attributes=SECURITY_ATTRIBUTES{nLength:std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,lpSecurityDescriptor:descriptor,bInheritHandle:0};
    let handle=unsafe{CreateNamedPipeW(wide(&format!("\\\\.\\pipe\\{name}")).as_ptr(),PIPE_ACCESS_OUTBOUND|FILE_FLAG_OVERLAPPED|FILE_FLAG_FIRST_PIPE_INSTANCE,
        PIPE_TYPE_BYTE|PIPE_READMODE_BYTE|PIPE_REJECT_REMOTE_CLIENTS,1,4096,4096,0,&attributes)};
    unsafe{LocalFree(descriptor);}
    if handle==INVALID_HANDLE_VALUE{return Err(win::error("无法创建 SSH 密码通道"));}
    let value=handle as usize;
    let worker=std::thread::spawn(move||{
        let mut stream=crate::approved::pipe::Stream{handle:Handle(value as HANDLE)};
        let event=Handle(unsafe{CreateEventW(null(),1,0,null())});
        if event.0.is_null(){return Err(win::error("创建密码通道事件失败"));}
        let mut overlapped:OVERLAPPED=unsafe{std::mem::zeroed()};overlapped.hEvent=event.0;
        let success=unsafe{ConnectNamedPipe(stream.handle.0,&mut overlapped)};
        let error=unsafe{GetLastError()};
        if success==0&&error!=ERROR_PIPE_CONNECTED{
            if error!=ERROR_IO_PENDING{return Err(win::error("SSH 密码通道连接失败"));}
            if unsafe{WaitForSingleObject(event.0,25000)}!=WAIT_OBJECT_0{
                unsafe{CancelIoEx(stream.handle.0,&overlapped);WaitForSingleObject(event.0,INFINITE);}
                return Err("SSH 密码通道超时。".into());
            }
        }
        stream.write_frame(password.as_bytes())
    });
    Ok((name,worker))
}
pub fn askpass()->Result<()>{
    let name=std::env::var("CORP_RUST_ASKPASS_PIPE").map_err(|_|"缺少密码通道。")?;
    if !name.starts_with("CorpRustSshAskpass_")||name.len()!=51{return Err("密码通道名称无效。".into());}
    let mut input=win::io(fs::File::open(format!("\\\\.\\pipe\\{name}")))?;
    let mut header=[0;4];win::io(input.read_exact(&mut header))?;
    let length=u32::from_le_bytes(header) as usize;if length==0||length>4096{return Err("密码长度无效。".into());}
    let mut bytes=Zeroizing::new(vec![0;length]);win::io(input.read_exact(&mut bytes))?;
    win::io(std::io::stdout().write_all(&bytes))?;win::io(std::io::stdout().write_all(b"\n"))
}
fn ssh(host:&str,credentials:&Credentials,id:&str,cancel:&AtomicBool)->Result<String>{
    let known=trusted_ssh(host,cancel)?;
    let (pipe,_server)=askpass_server(Zeroizing::new(credentials.password.clone()))?;
    let mut command=Command::new("ssh.exe");
    command.env("SSH_ASKPASS",win::current_exe()?).env("SSH_ASKPASS_REQUIRE","force").env("DISPLAY","corprust")
        .env("CORP_RUST_ASKPASS_PIPE",pipe)
        .args(["-T","-o","ConnectTimeout=20","-o","NumberOfPasswordPrompts=1","-o","PreferredAuthentications=password",
            "-o","StrictHostKeyChecking=yes","-o"]).arg(format!("UserKnownHostsFile={}",known.display()))
        .arg("-l").arg(&credentials.account).arg(host)
        .arg(format!("powershell.exe -NoLogo -NoProfile -NonInteractive -OutputFormat Text -EncodedCommand {}",encoded(&script(id))));
    output(command,None,Duration::from_secs(110),cancel)
}
fn winrm(host:&str,port:u16,credentials:&Credentials,id:&str,cancel:&AtomicBool)->Result<String>{
    let https=port==5986;
    if !https&&host.parse::<std::net::IpAddr>().is_ok(){return Err("HTTP WinRM 需要域主机名和 Kerberos。".into());}
    let uri=format!("{}://{host}:{port}/wsman",if https{"https"}else{"http"});
    let input=Zeroizing::new(serde_json::to_vec(&json!({"account":credentials.account,"password":credentials.password,"uri":uri,"https":https,"script":script(id)})).map_err(|e|e.to_string())?);
    let wrapper=r#"
$ErrorActionPreference='Stop'
$ProgressPreference='SilentlyContinue'
$data=[Console]::In.ReadToEnd() | ConvertFrom-Json
$password=ConvertTo-SecureString $data.password -AsPlainText -Force
$credential=[Management.Automation.PSCredential]::new($data.account,$password)
$authentication=if($data.https){'Negotiate'}else{'Kerberos'}
Invoke-Command -ConnectionUri $data.uri -Credential $credential -Authentication $authentication -ScriptBlock ([ScriptBlock]::Create($data.script))
"#;
    let mut command=Command::new(powershell());command.args(["-NoLogo","-NoProfile","-NonInteractive","-EncodedCommand",&encoded(wrapper)]);
    output(command,Some(input),Duration::from_secs(110),cancel)
}
pub(crate) fn probe_winrm(host:&str,port:u16,credentials:&Credentials,timeout:Duration,cancel:&AtomicBool)->Result<Value>{
    let https=port==5986;
    if !https&&host.parse::<std::net::IpAddr>().is_ok(){return Ok(json!({"available":null,"detail":"HTTP WinRM 需要域主机名和 Kerberos；未修改 TrustedHosts。"}));}
    let input=Zeroizing::new(serde_json::to_vec(&json!({"host":host,"port":port,"account":credentials.account,"password":credentials.password,"https":https})).map_err(|error|error.to_string())?);
    let script=r#"
$ErrorActionPreference='Stop'
$ProgressPreference='SilentlyContinue'
$data=[Console]::In.ReadToEnd() | ConvertFrom-Json
$credential=[Management.Automation.PSCredential]::new($data.account,(ConvertTo-SecureString $data.password -AsPlainText -Force))
$options=@{ComputerName=$data.host;Port=[int]$data.port;Credential=$credential;Authentication=$(if($data.https){'Negotiate'}else{'Kerberos'});ErrorAction='Stop'}
if($data.https){$options.UseSSL=$true}
$result=Test-WSMan @options
if(-not $result){throw 'No WSMan identity response.'}
Write-Output '{"available":true,"detail":"WSMan identity and authentication verified; execution privilege still requires validation."}'
"#;
    let mut command=Command::new(powershell());command.args(["-NoLogo","-NoProfile","-NonInteractive","-EncodedCommand",&encoded(script)]);
    serde_json::from_str(&output(command,Some(input),timeout,cancel)?).map_err(|_|"WinRM 预检回执无效。".into())
}
fn dcom(host:&str,credentials:&Credentials,id:&str,cancel:&AtomicBool)->Result<String>{
    // WMI Create runs in a noninteractive administrator session. Its receipt is
    // read through StdRegProv over DCOM, so this path does not depend on SMB 445.
    let target=format!("{}\n$receipt=$result | ConvertTo-Json -Compress -Depth 4\n$key='HKLM:\\SOFTWARE\\CorpInstallerRust\\RepairReceipts'\nNew-Item -Path $key -Force | Out-Null\nNew-ItemProperty -Path $key -Name '{id}' -Value $receipt -PropertyType String -Force | Out-Null",script(id));
    let input=Zeroizing::new(serde_json::to_vec(&json!({"host":host,"account":credentials.account,"password":credentials.password,"id":id,
        "command":format!("powershell.exe -NoLogo -NoProfile -NonInteractive -EncodedCommand {}",encoded(&target))})).map_err(|e|e.to_string())?);
    let wrapper=r#"
$ErrorActionPreference='Stop'
$ProgressPreference='SilentlyContinue'
$data=[Console]::In.ReadToEnd() | ConvertFrom-Json
$credential=[Management.Automation.PSCredential]::new($data.account,(ConvertTo-SecureString $data.password -AsPlainText -Force))
$options=@{ComputerName=$data.host;Credential=$credential;Authentication='PacketPrivacy';ErrorAction='Stop'}
$process=Get-WmiObject @options -Namespace root\cimv2 -List -Class Win32_Process
$registry=Get-WmiObject @options -Namespace root\default -List -Class StdRegProv
$launch=$process.Create($data.command,$null,$null)
if($launch.ReturnValue -ne 0){throw ('WMI process creation failed: '+$launch.ReturnValue)}
$deadline=[DateTime]::UtcNow.AddSeconds(90)
$key='SOFTWARE\CorpInstallerRust\RepairReceipts'
while([DateTime]::UtcNow -lt $deadline){
    $value=$registry.GetStringValue([uint32]2147483650,$key,$data.id)
    if($value.ReturnValue -eq 0 -and $value.sValue){
        $null=$registry.DeleteValue([uint32]2147483650,$key,$data.id)
        Write-Output ('CORP_CONNECTION_REPAIR:'+$value.sValue)
        exit 0
    }
    Start-Sleep -Milliseconds 500
}
throw 'WMI repair receipt timed out. The remote process may still be running; diagnose again.'
"#;
    let mut command=Command::new(powershell());command.args(["-NoLogo","-NoProfile","-NonInteractive","-EncodedCommand",&encoded(wrapper)]);
    output(command,Some(input),Duration::from_secs(110),cancel)
}
fn receipt(output:&str,id:&str)->Result<Value>{
    let records:Vec<_>=output.lines().filter_map(|line|line.trim().strip_prefix("CORP_CONNECTION_REPAIR:")).collect();
    if records.len()!=1{return Err("未收到唯一有效的修复回执，请重新诊断目标状态。".into());}
    let result:Value=serde_json::from_str(records[0]).map_err(|_|"修复回执格式无效。")?;
    if result["taskId"]!=id{return Err("修复回执 ID 不一致。".into());}
    Ok(result)
}
pub fn execute(host:&str,credentials:&Credentials,request:&Request,report:&crate::remote::Reporter,cancel:&AtomicBool)->Result<Value>{
    crate::model::host(host)?;
    let reachable=|port|->bool{use std::net::ToSocketAddrs;(host,port).to_socket_addrs().ok().into_iter().flatten().any(|address|
        std::net::TcpStream::connect_timeout(&address,Duration::from_millis(900)).is_ok())};
    let mut errors=Vec::new();let mut received=None;
    for port in [22,5986,5985,47001,135]{
        if cancel.load(Ordering::Relaxed){return Err("已取消修复。目标可能已部分修改，请重新检测。".into());}
        if !reachable(port){continue;}
        let channel=match port{22=>"SSH",5986=>"WinRM HTTPS",5985=>"WinRM HTTP",47001=>"WinRM 47001",_=>"WMI / DCOM"};
        report(format!("{host}：尝试通过 {channel} 验证管理员身份并下发共享连接修复。"));
        let outcome=if port==22{ssh(host,credentials,&request.id,cancel)}else if port==135{dcom(host,credentials,&request.id,cancel)}else{winrm(host,port,credentials,&request.id,cancel)};
        match outcome{
            Ok(output)=>{let mut result=receipt(&output,&request.id)?;result["channel"]=json!(channel);received=Some(result);break;},
            Err(error)=>{
                // Do not run another copy after a timeout: the previous remote
                // command may already have started and its outcome is unknown.
                if error.contains("超时")||error.contains("timed out"){return Err(error);}
                report(format!("{channel} 不可用：{error}"));errors.push(format!("{channel}: {error}"));
            }
        }
    }
    let mut result=received.ok_or_else(||format!("没有通过验证的修复通道。SSH、WinRM、WMI/DCOM 均不可用。{}",errors.join("；")))?;
    if result["success"]!=true{result["succeeded"]=json!(false);return Ok(result);}
    if result["changed"]!=true||result["stage"]!="Completed"||result["enabledRuleCount"].as_u64().unwrap_or(0)==0||
        !["LanmanServer","fdPHost","FDResPub"].iter().all(|service|result["runningServices"].as_array().is_some_and(|services|services.iter().any(|s|s.as_str().is_some_and(|s|s.eq_ignore_ascii_case(service))))){
        return Err("目标未确认所有规则和服务已就绪。".into());
    }
    let mut smb=false;
    for _ in 0..3{if cancel.load(Ordering::Relaxed){return Err("已取消后续检测。".into());}if reachable(445){smb=true;break;}std::thread::sleep(Duration::from_secs(1));}
    result["smbReachable"]=json!(smb);
    if !smb{result["succeeded"]=json!(false);result["partialSuccess"]=json!(true);result["message"]=json!("修复已执行，但 TCP 445 仍不通，请检查域策略和网络拦截。");}
    Ok(result)
}
#[cfg(test)]mod tests{use super::*;#[test]fn receipts_require_unique_matching_task(){
    assert!(receipt("CORP_CONNECTION_REPAIR:{\"taskId\":\"right\"}","right").is_ok());
    assert!(receipt("CORP_CONNECTION_REPAIR:{\"taskId\":\"other\"}","right").is_err());
    assert!(receipt("not a repair receipt","right").is_err());
    assert!(receipt("CORP_CONNECTION_REPAIR:{\"taskId\":\"right\"}\nCORP_CONNECTION_REPAIR:{\"taskId\":\"right\"}","right").is_err());
}}

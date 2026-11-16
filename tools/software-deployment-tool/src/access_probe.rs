use crate::{model::{self,Credentials,Result},win::{self,wide,NetworkIdentity}};
use serde_json::{Value,json};
use std::{io::{Read,Write},ptr::null,sync::atomic::AtomicBool,time::Duration};
use windows_sys::Win32::System::Services::*;
use zeroize::Zeroizing;
pub fn child(host:&str,probe_share:bool,wmi_only:bool)->Result<()>{
    model::host(host)?;
    let mut input=Zeroizing::new(Vec::new());win::io(std::io::stdin().take(65537).read_to_end(&mut input))?;
    if input.len()>65536{return Err("凭据输入过长。".into());}
    let credentials:Credentials=serde_json::from_slice(&input).map_err(|_|"凭据输入无效。")?;
    let _identity=NetworkIdentity::new(&credentials)?;
    if wmi_only{
        // The crate forwards its fourth argument directly to ConnectServer's
        // authority field; a bare domain there is invalid. Supply DOMAIN\user
        // as the username and leave authority empty.
        let account=credentials.account.strip_prefix(".\\").map(|user|format!("{host}\\{user}")).unwrap_or_else(||credentials.account.clone());
        let outcome=wmi::WMIConnection::with_credentials(host,Some(&account),Some(&credentials.password),None).map_err(|error|{
            let detail=error.to_string();
            if detail.contains("80070005"){format!("WMI 拒绝访问：请核对管理员权限、本地账号远程 UAC 过滤及 DCOM/WMI 命名空间权限。{detail}")}
            else if detail.to_ascii_lowercase().contains("800706ba"){format!("WMI 的 RPC 通道不可用：135 可达还需动态 RPC 通道和对应防火墙规则。{detail}")}
            else{detail}
        })
            .and_then(|connection|crate::inventory::query(&connection,"SELECT CSName,Version FROM Win32_OperatingSystem"));
        let value=match outcome{Ok(value)=>json!({"available":true,"rows":value}),Err(error)=>json!({"available":false,"error":error})};
        return win::io(std::io::stdout().write_all(&serde_json::to_vec(&value).map_err(|error|error.to_string())?));
    }
    let admin=if probe_share{match std::fs::read_dir(format!("\\\\{host}\\ADMIN$")){Ok(_)=>json!({"available":true}),Err(error)=>json!({"available":false,"code":error.raw_os_error(),"error":error.to_string()})}}else{json!({"available":null,"detail":"445 TCP 不通，未尝试 SMB 身份验证。"})};
    if admin["code"].as_i64().is_some_and(|code|matches!(code,1326|1327|1328|1329|1330|1331|1385|1793|1907|1909)){
        return win::io(std::io::stdout().write_all(&serde_json::to_vec(&json!({"adminShare":admin,"serviceManagement":{"available":null,"detail":"凭据验证被拒绝，未继续重复登录。"},"authenticationRejected":true})).map_err(|error|error.to_string())?));
    }
    let scm=unsafe{OpenSCManagerW(wide(&format!("\\\\{host}")).as_ptr(),null(),SC_MANAGER_CONNECT|SC_MANAGER_CREATE_SERVICE)};
    let service=if scm.is_null(){let error=std::io::Error::last_os_error();json!({"available":false,"code":error.raw_os_error(),"error":error.to_string()})}else{unsafe{CloseServiceHandle(scm);}json!({"available":true})};
    win::io(std::io::stdout().write_all(&serde_json::to_vec(&json!({"adminShare":admin,"serviceManagement":service})).map_err(|e|e.to_string())?))
}
pub fn wmi(host:&str,credentials:&Credentials,timeout:Duration,cancel:&AtomicBool)->Result<Value>{
    let mut command=std::process::Command::new(win::current_exe()?);command.args(["--access-probe",host,"--wmi-only"]);
    let input=Zeroizing::new(serde_json::to_vec(credentials).map_err(|error|error.to_string())?);
    let text=crate::repair::output(command,Some(input),timeout,cancel)?;serde_json::from_str(&text).map_err(|_|"WMI 预检回执无效。".into())
}
pub fn run(host:&str,credentials:&Credentials,probe_share:bool,timeout:Duration,cancel:&AtomicBool)->Result<Value>{
    let mut command=std::process::Command::new(win::current_exe()?);command.args(["--access-probe",host]);
    if !probe_share{command.arg("--rpc-only");}
    let input=Zeroizing::new(serde_json::to_vec(credentials).map_err(|e|e.to_string())?);
    let text=crate::repair::output(command,Some(input),timeout,cancel).map_err(|error|if error.contains("取消或超时"){"权限预检取消或超时；该预检只读，未修改目标配置。".into()}else{error})?;
    serde_json::from_str(&text).map_err(|_|"权限预检未返回有效结果。".into())
}

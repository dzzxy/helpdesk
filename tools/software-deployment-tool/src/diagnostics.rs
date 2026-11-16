use crate::{model::{self,Result},remote::Reporter};
use serde_json::{Value,json};
use std::{net::{IpAddr,SocketAddr,TcpStream,ToSocketAddrs},sync::atomic::{AtomicBool,Ordering},time::{Duration,Instant}};

fn ping(ip:IpAddr,timeout:Duration)->Value{
    let IpAddr::V4(ip)=ip else{return json!({"reachable":null,"detail":"当前原生 ICMP 检测仅支持 IPv4；TCP 检测仍支持 IPv6。"});};
    unsafe{
        use windows_sys::Win32::{Foundation::INVALID_HANDLE_VALUE,NetworkManagement::IpHelper::*};
        let handle=IcmpCreateFile();if handle==INVALID_HANDLE_VALUE{return json!({"reachable":null,"detail":std::io::Error::last_os_error().to_string()});}
        let data=b"CorpInstaller connectivity";let mut reply=vec![0u64;128];
        let count=IcmpSendEcho(handle,u32::from_ne_bytes(ip.octets()),data.as_ptr().cast(),data.len() as u16,std::ptr::null(),reply.as_mut_ptr().cast(),(reply.len()*8) as u32,timeout.as_millis() as u32);
        let error=std::io::Error::last_os_error().to_string();IcmpCloseHandle(handle);
        if count==0{return json!({"reachable":false,"detail":error});}
        let result=&*(reply.as_ptr() as *const ICMP_ECHO_REPLY);
        json!({"reachable":result.Status==0,"elapsedMs":result.RoundTripTime,"status":result.Status})
    }
}
pub fn diagnose(host:&str,seconds:u32,credentials:&crate::model::Credentials,report:&Reporter,cancel:&AtomicBool)->Result<Value>{
    model::host(host)?;let timeout=Duration::from_secs(seconds.clamp(1,30) as u64);
    let started=Instant::now();
    let mut addresses=(host,0).to_socket_addrs().map_err(|error|format!("DNS / 主机名解析失败：{error}"))?.map(|address|address.ip()).collect::<Vec<_>>();
    addresses.sort();addresses.dedup();if addresses.is_empty(){return Err("主机名没有解析到 IP 地址。".into());}
    let dns_elapsed=started.elapsed().as_millis();
    report(format!("{host}：解析为 {}",addresses.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ")));
    let (ping,ports)=std::thread::scope(|scope|{
        let ping=scope.spawn(||ping(addresses[0],timeout));
        let workers=[(445,"SMB"),(139,"NetBIOS SMB"),(135,"RPC / WMI"),(22,"SSH"),(5986,"WinRM HTTPS"),(5985,"WinRM HTTP"),(47001,"WinRM alternate"),(3389,"RDP")].into_iter().map(|(port,protocol)|{
            let addresses=&addresses;
            scope.spawn(move||{
                let start=Instant::now();let mut reachable=false;let mut last=String::new();
                for ip in addresses.iter().take(4){
                    if cancel.load(Ordering::Relaxed){break;}
                    let remaining=timeout.saturating_sub(start.elapsed());if remaining.is_zero(){break;}
                    match TcpStream::connect_timeout(&SocketAddr::new(*ip,port),remaining){Ok(_)=>{reachable=true;break;},Err(error)=>last=error.to_string()}
                }
                report(format!("{host}:{port} {protocol} {} · {} ms",if reachable{"可连接"}else{"不可连接"},start.elapsed().as_millis()));
                json!({"port":port,"protocol":protocol,"reachable":reachable,"elapsedMs":start.elapsed().as_millis(),"error":if reachable{String::new()}else{last}})
            })
        }).collect::<Vec<_>>();
        (ping.join().unwrap_or_else(|_|json!({"reachable":null,"detail":"ICMP 检测线程异常"})),workers.into_iter().filter_map(|worker|worker.join().ok()).collect::<Vec<_>>())
    });
    if cancel.load(Ordering::Relaxed){return Err("已取消连接诊断。".into());}
    let open=|port|ports.iter().any(|row|row["port"]==port&&row["reachable"]==true);
    let mut channels=Vec::new();
    for (port,name) in [(22,"SSH"),(5986,"WinRM HTTPS"),(5985,"WinRM HTTP"),(47001,"WinRM 47001"),(135,"WMI / DCOM（还需动态 RPC）")]{if open(port){channels.push(name);}}
    let guidance=if channels.is_empty(){"没有发现候选远程执行通道。需通过本地、已有远程管理工具或网络策略恢复一个通道。"}else{"端口开放仅表示候选通道；执行修复时会验证登录、管理员权限和修复回执。135 还依赖动态 RPC；139 和 3389 仅作诊断，不作为自动修复通道。"};
    let checks=if !credentials.account.is_empty()&&!credentials.password.is_empty(){
        report(format!("{host}：并行只读验证管理共享、服务管理、WMI 和 WinRM，不创建服务、不修改配置。"));
        let normalize=|result:Result<Value>|result.unwrap_or_else(|error|json!({"available":false,"error":if error.contains("取消或超时"){"只读预检取消或超时，未修改配置。".to_owned()}else{error}}));
        let primary=if open(445){("management".to_owned(),normalize(crate::access_probe::run(host,credentials,true,timeout,cancel)))}
            else if open(135){("wmi".to_owned(),normalize(crate::access_probe::wmi(host,credentials,timeout,cancel)))}
            else if let Some(port)=[5986,5985,47001].into_iter().find(|port|open(*port)){(format!("winrm{port}"),normalize(crate::repair::probe_winrm(host,port,credentials,timeout,cancel)))}
            else{("unavailable".into(),Value::Null)};
        let verified=primary.1["available"]==true||primary.1["adminShare"]["available"]==true||primary.1["serviceManagement"]["available"]==true;
        let mut checks=json!({"authenticationVerified":verified});
        let merge=|checks:&mut Value,key:String,value:Value|{
            if key=="management"&&value["adminShare"].is_object(){checks["adminShare"]=value["adminShare"].clone();checks["serviceManagement"]=value["serviceManagement"].clone();}
            else{checks[key]=value;}
        };
        if verified{
            std::thread::scope(|scope|{
                let mut workers=Vec::new();
                if (open(445)||open(135))&&primary.0!="management"{let smb=open(445);workers.push(scope.spawn(move||("management".to_owned(),normalize(crate::access_probe::run(host,credentials,smb,timeout,cancel)))));}
                if open(135)&&primary.0!="wmi"{workers.push(scope.spawn(move||("wmi".to_owned(),normalize(crate::access_probe::wmi(host,credentials,timeout,cancel)))));}
                for port in [5986,5985,47001]{if open(port)&&primary.0!=format!("winrm{port}"){workers.push(scope.spawn(move||(format!("winrm{port}"),normalize(crate::repair::probe_winrm(host,port,credentials,timeout,cancel)))));}}
                for worker in workers{if let Ok((key,value))=worker.join(){merge(&mut checks,key,value);}}
            });
        }else{checks["detail"]=json!("尚未验证凭据，未继续对其他协议重复登录；请核对首个验证结果。");}
        merge(&mut checks,primary.0,primary.1);checks
    }else{Value::Null};
    if cancel.load(Ordering::Relaxed){return Err("已取消连接诊断。".into());}
    let protocol=if checks.is_null(){"TCP 已检测；未提供凭据，未验证远程执行权限。".into()}else{
        [("adminShare","ADMIN$"),("serviceManagement","SCM 管理权限"),("wmi","WMI 只读"),("winrm5986","WinRM HTTPS 认证"),("winrm5985","WinRM HTTP 认证"),("winrm47001","WinRM 47001 认证")].iter().map(|(key,label)|{
            let value=&checks[*key];match value["available"].as_bool(){Some(true)=>format!("{label}通过"),Some(false) if !value["code"].is_null()=>format!("{label}失败（{}）",value["code"]),Some(false)=>format!("{label}失败或超时"),None=>format!("{label}未验证")}
        }).collect::<Vec<_>>().join("；")
    };
    Ok(json!({"dns":{"addresses":addresses.iter().map(ToString::to_string).collect::<Vec<_>>().join(", "),"elapsedMs":dns_elapsed},"ping":ping,"ports":ports,"channels":channels,"guidance":guidance,"protocolChecks":checks,"protocolDetail":protocol}))
}

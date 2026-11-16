use super::*;
use std::{collections::HashMap,sync::mpsc,time::Duration};
pub(super) const MESSAGE:u32=WM_APP+25;
type Selections = HashMap<String, (u32, String)>;
pub(super) struct Prompt{pub rows:Vec<(String,Vec<(u32,String)>)>,pub reply:mpsc::Sender<Result<Selections>>}
pub(super) fn select(window:usize,targets:&[String],credentials:&Credentials,request:&Request,cancel:&AtomicBool,report:&Reporter)->Result<Selections>{
    let needed=matches!(request.operation.as_str(),"install"|"run"|"bcu-uninstall"|"elevation-create"|"elevation-preview"|"elevation-discover")
        ||request.operation=="distribute"&&request.distribution.destination_preset.starts_with("User");
    if !needed||request.session!=0{return Ok(HashMap::new());}
    use std::sync::{Mutex,atomic::AtomicUsize};
    let next=AtomicUsize::new(0);let collected=Mutex::new(Vec::new());
    std::thread::scope(|scope|{for _ in 0..4.min(targets.len()){scope.spawn(||{
        let _identity=match crate::win::NetworkIdentity::new(credentials){Ok(identity)=>identity,Err(error)=>{report(error);return;}};
        loop{
            if cancel.load(Ordering::Relaxed){break;}let index=next.fetch_add(1,Ordering::Relaxed);let Some(host)=targets.get(index)else{break;};
            report(format!("{host}：自动检测目标活动登录用户…"));
            let sessions=match win::sessions(Some(host)){Ok(value)=>value,Err(error)=>{report(format!("{host}：会话预检失败：{error}"));continue;}};
            let choices=sessions.as_array().into_iter().flatten().filter(|row|row["state"]=="Active").filter_map(|row|{
                let id=u32::try_from(row["sessionId"].as_u64()?).ok()?;let account=row["account"].as_str()?;
                (id>0).then(||(id,account.to_owned()))
            }).collect::<Vec<_>>();
            if let Ok(mut result)=collected.lock(){result.push((index,host.clone(),choices));}
        }
    });}});
    if cancel.load(Ordering::Relaxed){return Err("已取消会话选择。".into());}
    let mut collected=collected.into_inner().map_err(|_|"会话检测状态异常。")?;collected.sort_by_key(|(index,_,_)|*index);
    let mut selected=HashMap::new();let mut rows=Vec::new();
    for (_,host,choices) in collected{match choices.as_slice(){[choice]=>{selected.insert(host,choice.clone());},[]=>(),_=>rows.push((host,choices))}}
    for group in rows.chunks(6){
        let (sender,receiver)=mpsc::channel();let pointer=Box::into_raw(Box::new(Prompt{rows:group.to_vec(),reply:sender}));
        if unsafe{PostMessageW(window as HWND,MESSAGE,0,pointer as isize)}==0{unsafe{drop(Box::from_raw(pointer));}return Err("无法显示会话选择窗口。".into());}
        loop{
            if cancel.load(Ordering::Relaxed){return Err("已取消会话选择。".into());}
            match receiver.recv_timeout(Duration::from_millis(100)){Ok(result)=>{selected.extend(result?);break;},Err(mpsc::RecvTimeoutError::Timeout)=>(),Err(_)=>return Err("会话选择窗口已经关闭。".into())}
        }
    }
    Ok(selected)
}
pub(super) fn show(owner:HWND,prompt:Prompt,closing:bool){
    let outcome=if closing{Err("窗口正在关闭。".into())}else{(||->Result<Selections>{
        let fields=prompt.rows.iter().map(|(host,choices)|{
            let labels=choices.iter().map(|(id,account)|format!("{account} · 会话 {id}")).collect::<Vec<_>>();
            let labels=labels.iter().map(String::as_str).collect::<Vec<_>>();
            crate::form::Field::choice(host,&labels,0)
        }).collect();
        let Some(values)=crate::form::show(owner,"选择目标登录会话","以下电脑有多个活动会话。为每台电脑选择本次操作的使用者；只有一个活动会话的电脑已自动选择。",fields)?else{return Err("已取消会话选择，未下发本批操作。".into());};
        prompt.rows.iter().zip(values).map(|((host,choices),value)|{
            let index=value.parse::<usize>().map_err(|_|"未选择有效登录会话。")?;
            let choice=choices.get(index).ok_or("未选择有效登录会话。")?;
            Ok((host.clone(),choice.clone()))
        }).collect()
    })()};let _=prompt.reply.send(outcome);
}

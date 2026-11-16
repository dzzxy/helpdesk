use serde_json::Value;
use super::*;
fn text(value:&Value)->String{value.as_str().map(str::to_owned).unwrap_or_else(||if value.is_null(){String::new()}else{value.to_string()})}
pub(super) fn columns(page:i32)->&'static[(&'static str,i32)]{match page{
    0=>&[("目标电脑",140),("任务 ID",220),("状态",110),("进度",85),("退出码",85),("信息",360)],
    1=>&[("目标电脑",145),("状态",105),("进度",85),("已复制",80),("已跳过",80),("失败",70),("信息",360)],
    3=>&[("目标电脑",125),("状态",85),("制造商",120),("型号",150),("序列号",135),("操作系统",220),("当前用户",150),("信息",260)],
    4=>&[("目标电脑",160),("状态",110),("当前值",90),("结果",480)],
    5=>&[("目标电脑",140),("修复状态",100),("DNS",100),("Ping",85),("3389 RDP",95),("445 SMB",95),("135 RPC",95),("WinRM 端口",120),("22 SSH",95),("协议深度检测",180),("推荐通道",140),("修复结果",240),("详细信息",320)],
    7=>&[("软件名称",240),("版本",130),("发布者",180),("类型",100),("架构",80),("大小",90),("可卸载",110)],
    8=>&[("目标电脑",170),("任务 ID",230),("状态",120),("退出码",85),("信息",400)],
    9=>&[("目标电脑",150),("状态",100),("事件",80),("发现",80),("诊断包",110),("信息",320)],
    10=>&[("软件",160),("程序",400),("启用",65),("信息",300)],
    11=>&[("状态",110),("模块",150),("目标电脑",150),("任务",180),("进度",75),("创建时间",150),("耗时",95),("执行信息",500)],
    _=>&[("目标电脑",155),("状态",130),("操作",145),("信息",440)],
}}
pub(super) fn row(page:i32,row:&Value)->Vec<String>{
    let data=&row["data"];let host=text(&row["computer"]);let state=text(&row["state"]);let detail=text(&row["detail"]);
    match page{
        0=>vec![host,text(&row["id"]),state,if data["progressKind"]=="Stage"{text(&data["phase"])}else if matches!(data["state"].as_str(),Some("Succeeded"|"RebootRequired")){"100%".into()}else{String::new()},text(&data["exitCode"]),detail],
        1=>{
            let count=|name|data[name].as_u64().unwrap_or_default();
            let progress=data["progress"].as_u64().or_else(||{let total=count("totalFiles");(total>0).then(||(count("files")+count("skipped")+count("failed"))*100/total)});
            vec![host,state,progress.map(|p|format!("{p}%")).unwrap_or_default(),text(&data["files"]),text(&data["skipped"]),text(&data["failed"]),data["error"].as_str().unwrap_or(&detail).into()]
        },
        3=>vec![host,state,text(&data["system"][0]["Manufacturer"]),text(&data["system"][0]["Model"]),text(&data["bios"][0]["SerialNumber"]),text(&data["operatingSystem"][0]["Caption"]),text(&data["system"][0]["UserName"]),detail],
        4=>vec![host,state,text(&data["data"]),if data["written"]==true{"已写入并回读验证".into()}else if data["value"].is_string(){"检测完成".into()}else{detail}],
        5=>{
            let ports=data["ports"].as_array().or_else(||data.as_array());
            let status=|value:&Value|match value["reachable"].as_bool(){Some(true)=>format!("可达 {} ms",text(&value["elapsedMs"])),Some(false)=>"不可达".into(),None=>String::new()};
            let port=|number:u16|ports.and_then(|rows|rows.iter().find(|row|row["port"]==number)).map(&status).unwrap_or_default();
            let winrm=[5986,5985,47001].into_iter().filter_map(|number|{let value=port(number);(!value.is_empty()).then(||format!("{number}: {value}"))}).collect::<Vec<_>>().join("; ");
            vec![host,state,text(&data["dns"]["addresses"]),status(&data["ping"]),port(3389),port(445),port(135),winrm,port(22),text(&data["protocolDetail"]),data["channels"].as_array().map(|rows|rows.iter().map(text).collect::<Vec<_>>().join(" / ")).unwrap_or_default(),if data["smbReachable"]==true{"修复后 445 已通".into()}else{text(&data["message"])},if data["guidance"].is_string(){text(&data["guidance"])}else{detail}]
        },
        7 if data["displayName"].is_string()=>vec![text(&data["displayName"]),text(&data["version"]),text(&data["publisher"]),text(&data["uninstallerKind"]),text(&data["architecture"]),data["estimatedSize"].as_str().and_then(|value|value.trim().parse::<f64>().ok()).map(|kb|format!("{:.1} MB",kb/1024.0)).unwrap_or_default(),if data["interactive"]!=true{"缺少普通卸载命令".into()}else if data["protected"]==true{"受保护".into()}else if data["valid"]==false{"卸载程序无效".into()}else{"支持".into()}],
        7=>vec![if data["succeeded"]==true{"卸载 / 清理完成".into()}else{state},String::new(),String::new(),String::new(),String::new(),String::new(),data["error"].as_str().unwrap_or(&detail).into()],
        8=>vec![host,text(&row["id"]),state,if data["cleared"]==true{"0".into()}else{String::new()},if data["overwriteEnabled"]==true{"已清理，并验证按需覆盖".into()}else{detail}],
        9=>vec![host,state,text(&data["summary"]["eventCount"]),text(&data["summary"]["findingCount"]),text(&data["bundleBytes"]),data["error"].as_str().unwrap_or(&detail).into()],
        10=>vec![data["name"].as_str().unwrap_or(&state).into(),text(&data["target"]),if data["enabled"]==true{"是".into()}else{String::new()},data["error"].as_str().unwrap_or(&detail).into()],
        _=>vec![host,state,text(&row["operation"]),detail]
    }
}
pub(super) struct Refresh{window:HWND,horizontal:i32,selected:isize,top:isize,visible:bool}
impl Refresh{
    pub(super) fn begin(window:HWND)->Self{unsafe{
        let horizontal=GetScrollPos(window,SB_HORZ);let selected=SendMessageW(window,LVM_GETNEXTITEM,usize::MAX,LVNI_SELECTED as isize);let top=SendMessageW(window,LVM_GETTOPINDEX,0,0);
        let visible=GetWindowLongW(window,GWL_STYLE) as u32&WS_VISIBLE!=0;
        if visible{SendMessageW(window,WM_SETREDRAW,0,0);}Self{window,horizontal,selected,top,visible}
    }}
}
impl Drop for Refresh{fn drop(&mut self){unsafe{
    let count=SendMessageW(self.window,LVM_GETITEMCOUNT,0,0);
    if self.selected>=0&&self.selected<count{let item=LVITEMW{stateMask:LVIS_SELECTED|LVIS_FOCUSED,state:LVIS_SELECTED|LVIS_FOCUSED,..zeroed()};SendMessageW(self.window,LVM_SETITEMSTATE,self.selected as usize,(&item as *const LVITEMW) as isize);}
    if self.top>0&&self.top<count{
        let mut rect:RECT=zeroed();
        if SendMessageW(self.window,LVM_GETITEMRECT,0,(&mut rect as *mut RECT) as isize)!=0{
            let current=SendMessageW(self.window,LVM_GETTOPINDEX,0,0);
            SendMessageW(self.window,LVM_SCROLL,0,(self.top-current)*(rect.bottom-rect.top) as isize);
        }
    }
    let current=GetScrollPos(self.window,SB_HORZ);if current!=self.horizontal{SendMessageW(self.window,LVM_SCROLL,(self.horizontal-current) as usize,0);}
    if self.visible{SendMessageW(self.window,WM_SETREDRAW,1,0);RedrawWindow(self.window,null(),null_mut(),RDW_INVALIDATE|RDW_ERASE|RDW_FRAME|RDW_ALLCHILDREN|RDW_UPDATENOW);}
}}}
pub(super) fn timestamp(milliseconds:i64)->String{unsafe{
    use windows_sys::Win32::{Storage::FileSystem::FileTimeToLocalFileTime,System::Time::FileTimeToSystemTime};
    let ticks=((milliseconds.max(0) as u64)/1000+11644473600)*10000000;
    let utc=FILETIME{dwLowDateTime:ticks as u32,dwHighDateTime:(ticks>>32) as u32};let mut local=FILETIME::default();let mut value=SYSTEMTIME::default();
    if FileTimeToLocalFileTime(&utc,&mut local)==0||FileTimeToSystemTime(&local,&mut value)==0{return String::new();}
    format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}",value.wYear,value.wMonth,value.wDay,value.wHour,value.wMinute,value.wSecond)
}}
pub(super) fn task_state(state:&str)->&str{match state{"Queued"=>"排队中","Running"=>"执行中","Cancelling"=>"取消中","Cancelled"=>"已取消","Succeeded"=>"成功","PartialSuccess"=>"部分成功","Failed"=>"失败","Unconfirmed"=>"结果待确认","Interrupted"=>"管理端已中断",_=>state}}
pub(super) fn operation(operation:&str)->&str{match operation{
    "install"=>"软件安装","run"=>"直接运行","distribute"=>"文件夹分发","sessions"=>"检测登录会话","inventory"=>"电脑信息收集","registry-query"=>"检测当前值","registry-set"=>"修改固定值","diagnose"=>"连接诊断","connection-repair"=>"连接修复","bcu-scan"=>"扫描已安装软件","bcu-uninstall"=>"软件卸载","bcu-delete-junk"=>"卸载残留清理","security-clear"=>"安全日志维护","security-info"=>"检测日志状态","diagnostic-logs"=>"日志诊断收集","elevation-create"|"elevation-approve"=>"创建软件授权","elevation-preview"=>"预览软件详情","elevation-discover"=>"查找软件入口","elevation-list"=>"查看已有授权","elevation-revoke"=>"撤销软件授权",_=>operation
}}
pub(super) fn sort_key(value:&str)->(u8,u128,String){
    value.trim().trim_end_matches('%').parse::<u128>().map(|number|(0,number,String::new())).unwrap_or_else(|_|(1,0,value.to_lowercase()))
}
pub(super) fn shown_records<'a>(page:i32,records:&'a[Value],query:&str,sort:Option<(usize,bool)>)->Vec<&'a Value>{
    let mut records=records.iter().filter(|row|query.is_empty()||["displayName","version","publisher"].iter().any(|field|row["data"][field].as_str().unwrap_or_default().to_lowercase().contains(query))).collect::<Vec<_>>();
    if let Some((column,ascending))=sort{records.sort_by_cached_key(|record|sort_key(row(page,record).get(column).map(String::as_str).unwrap_or_default()));if !ascending{records.reverse();}}
    records
}
pub(super) fn history_row(entry:&crate::tasks::Entry)->Vec<String>{
    let duration=if matches!(entry.state.as_str(),"Queued"|"Running"|"Cancelling"){crate::tasks::timestamp()}else{entry.updated}.saturating_sub(entry.created).max(0)/1000;
    let title=if !entry.request.install.display_name.is_empty(){entry.request.install.display_name.clone()}else if !entry.request.bcu.display_name.is_empty(){entry.request.bcu.display_name.clone()}else if !entry.request.source.is_empty(){std::path::Path::new(&entry.request.source).file_name().unwrap_or_default().to_string_lossy().into_owned()}else{operation(&entry.operation).into()};
    let progress=if entry.state=="Succeeded"{100}else{entry.progress};
    vec![task_state(&entry.state).into(),operation(&entry.operation).into(),entry.target.clone(),title,format!("{progress}%"),timestamp(entry.created),format!("{duration} 秒"),entry.detail.clone()]
}
pub(super) fn header(window:HWND,columns:usize,sort:Option<(usize,bool)>){unsafe{
    let header=SendMessageW(window,LVM_GETHEADER,0,0) as HWND;
    for index in 0..columns{let mut item=HDITEMW{mask:HDI_FORMAT,..zeroed()};
        if SendMessageW(header,HDM_GETITEMW,index,(&mut item as *mut HDITEMW) as isize)!=0{
            item.fmt&=!(HDF_SORTUP|HDF_SORTDOWN) as i32;
            if let Some((column,ascending))=sort{if column==index{item.fmt|=if ascending{HDF_SORTUP}else{HDF_SORTDOWN} as i32;}}
            SendMessageW(header,HDM_SETITEMW,index,(&item as *const HDITEMW) as isize);
        }
    }
}}

#[cfg(test)]mod tests{
    use super::*;
    #[test]fn completed_history_displays_completion_without_changing_active_progress(){
        let mut entry=crate::tasks::Entry{id:"old-task".into(),target:"test-host".into(),operation:"inventory".into(),account:String::new(),state:"Succeeded".into(),detail:String::new(),created:0,updated:1000,progress:0,request:Request::default(),result:None};
        assert_eq!(history_row(&entry)[4],"100%");
        entry.state="Running".into();entry.progress=37;assert_eq!(history_row(&entry)[4],"37%");
        entry.state="Cancelled".into();assert_eq!(history_row(&entry)[4],"37%");
    }
    #[test]fn numeric_sort_preserves_target_record_identity(){
        let records=[serde_json::json!({"id":"task-b","computer":"host-b","data":{"files":10}}),serde_json::json!({"id":"task-a","computer":"host-a","data":{"files":2}})];
        let rows=shown_records(1,&records,"",Some((3,true)));assert_eq!(rows[0]["computer"],"host-a");assert_eq!(rows[0]["id"],"task-a");
        let rows=shown_records(1,&records,"",Some((3,false)));assert_eq!(rows[0]["computer"],"host-b");assert_eq!(rows[0]["id"],"task-b");
    }
    #[test]fn diagnostics_project_each_port_into_its_own_column(){
        let record=serde_json::json!({"computer":"example","state":"成功","operation":"diagnose","data":{"dns":{"addresses":"127.0.0.1"},"ping":{"reachable":true,"elapsedMs":1},"ports":[{"port":445,"reachable":false},{"port":22,"reachable":true,"elapsedMs":2}]}});
        let values=row(5,&record);assert_eq!(values.len(),columns(5).len());assert_eq!(values[2],"127.0.0.1");assert_eq!(values[5],"不可达");assert_eq!(values[8],"可达 2 ms");assert_eq!(values[6],"");
    }
}

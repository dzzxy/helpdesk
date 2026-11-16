use crate::{model::Result, win::{self,wide}};
use serde::{Serialize,Deserialize};
use serde_json::{Value,json};
use std::{fs,io::Write,mem::{size_of,zeroed},path::Path,ptr::{null,null_mut}};
use windows_sys::Win32::{Foundation::*,System::EventLog::*};

struct EventHandle(EVT_HANDLE);
impl Drop for EventHandle {fn drop(&mut self){unsafe{EvtClose(self.0);}}}
fn event_handle(handle:EVT_HANDLE,context:&str)->Result<EventHandle> {
    if handle==0 {Err(win::error(context))}else{Ok(EventHandle(handle))}
}
fn channel_bool(handle:EVT_HANDLE,property:i32)->Result<bool>{
    let mut value:EVT_VARIANT=unsafe{zeroed()};let mut used=0;
    unsafe{
        win::check(EvtGetChannelConfigProperty(handle,property,0,size_of::<EVT_VARIANT>() as u32,&mut value,&mut used),"读取日志配置失败")?;
        Ok(value.Anonymous.BooleanVal!=0)
    }
}
pub fn security_info()->Result<Value>{
    unsafe{
        let log=event_handle(EvtOpenLog(0,wide("Security").as_ptr(),EvtOpenChannelPath),"打开安全日志失败")?;
        let mut value:EVT_VARIANT=zeroed();let mut used=0;
        win::check(EvtGetLogInfo(log.0,EvtLogNumberOfLogRecords,size_of::<EVT_VARIANT>() as u32,&mut value,&mut used),"读取安全日志记录数失败")?;
        let count=value.Anonymous.UInt64Val;
        let config=event_handle(EvtOpenChannelConfig(0,wide("Security").as_ptr(),0),"读取安全日志配置失败")?;
        let mut capacity:EVT_VARIANT=zeroed();let mut capacity_used=0;
        win::check(EvtGetChannelConfigProperty(config.0,EvtChannelLoggingConfigMaxSize,0,size_of::<EVT_VARIANT>() as u32,&mut capacity,&mut capacity_used),"读取日志容量失败")?;
        Ok(json!({"records":count,"retention":channel_bool(config.0,EvtChannelLoggingConfigRetention)?,
            "autoBackup":channel_bool(config.0,EvtChannelLoggingConfigAutoBackup)?,"maximumBytes":capacity.Anonymous.UInt64Val}))
    }
}
pub fn clear_security()->Result<Value>{
    crate::admin::privilege("SeSecurityPrivilege")?;
    let before=security_info()?;
    unsafe{
        win::check(EvtClearLog(0,wide("Security").as_ptr(),null(),0),"清理安全日志失败")?;
        let config=event_handle(EvtOpenChannelConfig(0,wide("Security").as_ptr(),0),"打开安全日志配置失败")?;
        let value=EVT_VARIANT{Anonymous:EVT_VARIANT_0{BooleanVal:0},Count:0,Type:EvtVarTypeBoolean as u32};
        for property in [EvtChannelLoggingConfigRetention,EvtChannelLoggingConfigAutoBackup] {
            win::check(EvtSetChannelConfigProperty(config.0,property,0,&value),"日志已清理，但配置自动覆盖失败")?;
        }
        win::check(EvtSaveChannelConfig(config.0,0),"日志已清理，但保存自动覆盖配置失败")?;
    }
    let after=security_info()?;
    if after["retention"]!=false||after["autoBackup"]!=false {return Err("日志已清理，但自动覆盖设置回读失败。".into());}
    if before["maximumBytes"]!=after["maximumBytes"]{return Err("日志已清理，但日志容量与维护前不一致，请核对策略。".into());}
    Ok(json!({"before":before,"after":after,"cleared":true,"overwriteEnabled":true}))
}

#[derive(Clone,Debug,Serialize,Deserialize)]
#[serde(default)]
pub struct Options{
    pub days:u32,pub performance:bool,pub blue_screen:bool,pub crash:bool,pub installation:bool,
    pub events:bool,pub reliability:bool,pub evtx:bool,pub dumps:bool,pub security:bool,
}
impl Default for Options{
    fn default()->Self{Self{days:7,performance:true,blue_screen:true,crash:true,installation:true,events:true,reliability:true,evtx:true,dumps:true,security:false}}
}
fn read_channel(channel:&str,query:&str,remaining:&mut usize)->Result<(Vec<Value>,bool)>{
    unsafe{
        let result=event_handle(EvtQuery(0,wide(channel).as_ptr(),wide(query).as_ptr(),(EvtQueryChannelPath|EvtQueryReverseDirection) as u32),"读取事件通道失败")?;
        let mut rows=Vec::new();let mut truncated=false;
        for index in 0..2000 {
            let mut event=0;let mut count=0;
            if EvtNext(result.0,1,&mut event,1000,0,&mut count)==0{
                if GetLastError()==ERROR_NO_MORE_ITEMS{break;}
                return Err(win::error("枚举事件失败"));
            }
            let event=EventHandle(event);let mut required=0;let mut properties=0;
            EvtRender(0,event.0,EvtRenderEventXml as u32,0,null_mut(),&mut required,&mut properties);
            if required>4*1024*1024{return Err("事件 XML 过大。".into());}
            let reservation=(required as usize).saturating_mul(2).saturating_add(1024);
            if reservation>*remaining{truncated=true;break;}
            *remaining-=reservation;
            let mut buffer=vec![0u16;(required as usize).div_ceil(2)];
            win::check(EvtRender(0,event.0,EvtRenderEventXml as u32,required,buffer.as_mut_ptr().cast(),&mut required,&mut properties),"读取事件 XML 失败")?;
            let xml=String::from_utf16_lossy(&buffer).trim_end_matches('\0').to_owned();
            let doc=roxmltree::Document::parse(&xml).map_err(|e|e.to_string())?;
            let field=|name:&str|doc.descendants().find(|node|node.has_tag_name(name)).and_then(|node|node.text()).unwrap_or("").to_owned();
            let provider=doc.descendants().find(|node|node.has_tag_name("Provider")).and_then(|node|node.attribute("Name")).unwrap_or("");
            let time=doc.descendants().find(|node|node.has_tag_name("TimeCreated")).and_then(|node|node.attribute("SystemTime")).unwrap_or("");
            rows.push(json!({"channel":channel,"eventId":field("EventID"),"level":field("Level"),"provider":provider,"time":time,"xml":xml}));
            if index==1999{truncated=true;}
        }
        Ok((rows,truncated))
    }
}
pub fn diagnostic(options:&Options,directory:&Path)->Result<Value>{
    if ![1,3,7,30].contains(&options.days){return Err("日志范围无效。".into());}
    let mut channels=vec!["Application","System","Setup","ForwardedEvents"];
    if options.security{channels.push("Security");}
    if options.performance{channels.push("Microsoft-Windows-Diagnostics-Performance/Operational");}
    if options.installation{channels.extend(["Microsoft-Windows-AppLocker/EXE and DLL","Microsoft-Windows-CodeIntegrity/Operational"]);}
    let query=format!("*[System[TimeCreated[timediff(@SystemTime) <= {}]]]",options.days as u64*86400000);
    let mut events=Vec::new();let mut warnings=Vec::new();let mut archives=Vec::new();let mut event_memory_budget=64*1024*1024usize;
    for (index,channel) in channels.iter().enumerate(){
        if directory.join("cancel.signal").exists(){return Err("日志收集已取消。".into());}
        if options.events{match read_channel(channel,&query,&mut event_memory_budget){Ok((rows,truncated))=>{events.extend(rows);if truncated{warnings.push(format!("{channel}：达到每通道 2000 项或总事件内存 64 MiB 限额，结构化事件已截断；EVTX 仍按选项导出。"));}},Err(error)=>warnings.push(format!("{channel}: {error}"))}}
        if options.evtx{
            let file=directory.join(format!("channel-{index}.evtx"));
            if unsafe{EvtExportLog(0,wide(channel).as_ptr(),wide(&query).as_ptr(),wide(&file.to_string_lossy()).as_ptr(),EvtExportLogChannelPath as u32)}!=0{
                archives.push((format!("{}.evtx",channel.replace(['/','\\'],"_")),file));
            }else{warnings.push(win::error(&format!("导出 {channel} 失败")));}
        }
    }
    let reliability=if options.reliability{
        let cutoff=wmi_cutoff(options.days)?;
        match wmi::WMIConnection::new().map_err(|e|e.to_string()).and_then(|connection|
            crate::inventory::query(&connection,&format!("SELECT SourceName,ProductName,EventIdentifier,TimeGenerated,Message FROM Win32_ReliabilityRecords WHERE TimeGenerated >= '{cutoff}'"))){
            Ok(mut data)=>{if let Some(rows)=data.as_array_mut(){rows.sort_by(|a,b|b["TimeGenerated"].as_str().cmp(&a["TimeGenerated"].as_str()));rows.truncate(2000);}data},Err(error)=>{warnings.push(error);json!([])}
        }
    }else{json!([])};
    if options.dumps{
        let dumps=Path::new(&std::env::var("SystemRoot").unwrap_or("C:\\Windows".into())).join("Minidump");
        if let Ok(entries)=fs::read_dir(dumps){
            for entry in entries.flatten(){
                if entry.path().extension().is_none_or(|ext|!ext.eq_ignore_ascii_case("dmp")){continue;}
                if let Ok(metadata)=entry.metadata(){
                    let recent=metadata.modified().ok().and_then(|t|t.elapsed().ok()).is_some_and(|age|age.as_secs()<options.days as u64*86400);
                    if recent{archives.push((format!("Minidump/{}",entry.file_name().to_string_lossy()),entry.path()));}
                }
            }
        }
    }
    let findings=findings(&events);
    let mut summary=json!({"computer":std::env::var("COMPUTERNAME").unwrap_or_default(),"rangeDays":options.days,"eventCount":events.len(),"findingCount":findings.len(),"findings":findings,
        "critical":events.iter().filter(|e|e["level"]=="1").count(),"errors":events.iter().filter(|e|e["level"]=="2").count(),
        "warnings":warnings,"archiveFiles":0,"reliabilityRecordCount":reliability.as_array().map_or(0,Vec::len)});
    let file=win::io(fs::File::create(directory.join("diagnostic.zip")))?;
    let mut zip=zip::ZipWriter::new(file);let config=zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    const MAXIMUM:u64=200*1024*1024;
    let mut total=0u64;
    for (name,data) in [("events.json",json!(events)),("reliability.json",reliability)]{
        let bytes=serde_json::to_vec_pretty(&data).map_err(|e|e.to_string())?;
        total=total.checked_add(bytes.len() as u64).ok_or("日志总大小溢出。")?;
        if total>MAXIMUM{return Err("结构化日志超过 200 MB，请缩小时间范围。".into());}
        zip.start_file(name,config).map_err(|e|e.to_string())?;
        win::io(zip.write_all(&bytes))?;
    }
    let mut included=0;
    for (name,path) in archives{
        if directory.join("cancel.signal").exists(){return Err("日志收集已取消。".into());}
        let size=win::io(fs::metadata(&path))?.len();
        if total+size+1024*1024>MAXIMUM {summary["warnings"].as_array_mut().unwrap().push(json!(format!("超过 200 MB 包大小限制，跳过 {name}")));continue;}
        zip.start_file(name,config).map_err(|e|e.to_string())?;
        use std::io::Read;
        let copied=win::io(std::io::copy(&mut win::io(fs::File::open(path))?.take(MAXIMUM-total+1),&mut zip))?;
        total+=copied;if total>MAXIMUM{return Err("收集期间日志文件增长，超过 200 MB 上限。".into());}included+=1;
    }
    summary["archiveFiles"]=json!(included);
    zip.start_file("summary.json",config).map_err(|e|e.to_string())?;
    win::io(zip.write_all(&serde_json::to_vec_pretty(&summary).map_err(|e|e.to_string())?))?;
    let file=zip.finish().map_err(|e|e.to_string())?;let size=win::io(file.metadata())?.len();
    if size>MAXIMUM{return Err("最终诊断包超过 200 MB，请缩小时间范围。".into());}
    Ok(json!({"summary":summary,"bundle":"diagnostic.zip","bundleBytes":size}))
}
fn wmi_cutoff(days:u32)->Result<String>{
    use windows_sys::Win32::System::Time::FileTimeToSystemTime;
    let millis=crate::tasks::timestamp().saturating_sub(days as i64*86400000);let ticks=(millis as u64+11644473600000)*10000;
    let file=FILETIME{dwLowDateTime:ticks as u32,dwHighDateTime:(ticks>>32) as u32};let mut time=SYSTEMTIME::default();
    unsafe{win::check(FileTimeToSystemTime(&file,&mut time),"转换日志开始时间失败")?;}
    Ok(format!("{:04}{:02}{:02}{:02}{:02}{:02}.000000+000",time.wYear,time.wMonth,time.wDay,time.wHour,time.wMinute,time.wSecond))
}
fn findings(events:&[Value])->Vec<Value>{
    let mut result=Vec::new();
    for category in 0..5{
        let matches=events.iter().filter(|event|{
            let id=event["eventId"].as_str().and_then(|s|s.parse::<u32>().ok()).unwrap_or_default();let provider=event["provider"].as_str().unwrap_or_default().to_ascii_lowercase();
            let any=|parts:&[&str]|parts.iter().any(|part|provider.contains(part));
            match category{
                0=>matches!(id,41|6008)||id==1001&&any(&["bugcheck","wer-systemerrorreporting"]),
                1=>any(&["whea","disk","ntfs","stornvme","storahci"])&&matches!(id,7|11|15|17|18|19|20|51|55|129|153),
                2=>matches!(id,1000|1001|1002|1026)&&any(&["application",".net runtime","windows error reporting"]),
                3=>id==2004||any(&["resource-exhaustion","diagnostics-performance"]),
                _=>any(&["msiinstaller","applocker","codeintegrity"])||matches!(id,11708|10005|10010)
            }
        }).collect::<Vec<_>>();
        if matches.is_empty(){continue;}
        let newest=matches.iter().max_by_key(|event|event["time"].as_str().unwrap_or_default()).unwrap();
        let (name,title)=match category{0=>("蓝屏/异常重启","检测到异常关机、BugCheck 或非正常重启记录"),1=>("硬件与存储","检测到硬件、磁盘、文件系统或存储超时事件"),2=>("软件崩溃","检测到应用程序崩溃、无响应或 .NET 异常"),3=>("性能卡顿","检测到资源耗尽或系统性能诊断事件"),_=>("安装失败","检测到安装、应用控制或代码完整性相关事件")};
        result.push(json!({"severity":if category<2{"High"}else{"Medium"},"category":name,"title":title,"count":matches.len(),"latestAtUtc":newest["time"],"details":format!("最近：{} / Event ID {}",newest["provider"].as_str().unwrap_or_default(),newest["eventId"].as_str().unwrap_or_default())}));
    }result
}

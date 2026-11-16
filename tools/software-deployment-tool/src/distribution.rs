use crate::model::Result;
use serde::{Serialize,Deserialize};
use crate::{model::{self,Credentials,Request,ConflictPolicy},remote::Reporter,win::{self,NetworkIdentity,wide}};
use serde_json::{Value,json};
use sha2::{Digest,Sha256};
use std::{fs,io::{Read,Write,Seek,SeekFrom},path::{Path,PathBuf},sync::{Mutex,atomic::{AtomicBool,AtomicUsize,Ordering}},time::Duration};
use std::os::windows::{fs::{MetadataExt,OpenOptionsExt},io::AsRawHandle};
use windows_sys::Win32::{Foundation::FILETIME,Storage::FileSystem::*};

#[derive(Clone,Debug,Serialize,Deserialize)]
#[serde(default,rename_all="camelCase")]
pub struct Options{
    pub source_computer:String,pub source_account:String,pub destination_preset:String,
    pub user_account:String,pub target_concurrency:usize,pub file_concurrency:usize,
    pub full_verification:bool,pub include_files:String,pub exclude_files:String,
    pub exclude_directories:String,pub maximum_file_bytes:Option<u64>,
}
impl Default for Options{fn default()->Self{Self{
    source_computer:String::new(),source_account:String::new(),destination_preset:"Custom".into(),
    user_account:String::new(),target_concurrency:4,file_concurrency:4,full_verification:false,
    include_files:String::new(),exclude_files:String::new(),exclude_directories:String::new(),maximum_file_bytes:None,
}}}
impl Options{
    pub fn validate(&self)->Result<()>{
        if !(1..=16).contains(&self.target_concurrency)||!(1..=16).contains(&self.file_concurrency){return Err("分发并发须在 1～16 之间。".into());}
        if !["Custom","PublicDesktop","PublicStartMenu","ProgramDataCompany","UserDesktop","UserDocuments","UserDownloads"].contains(&self.destination_preset.as_str()){
            return Err("分发目标预设无效。".into());
        }
        Ok(())
    }
}
pub fn high_risk(path:&str)->bool{
    let path=path.trim_end_matches('\\').to_ascii_lowercase();
    path.len()<=2||["\\windows","\\program files","\\program files (x86)","\\users","\\system volume information","\\$recycle.bin","\\corpinstallertemp","\\corpinstallerrusttemp"]
        .iter().any(|part|path[2.min(path.len())..]==**part||path[2.min(path.len())..].starts_with(&format!("{part}\\")))
}

const EXECUTABLES:[&str;14]=["exe","msi","msp","mst","cmd","bat","ps1","vbs","vbe","js","jse","wsf","scr","com"];
struct Source<'a>{root:PathBuf,credentials:Option<&'a Credentials>}
impl Source<'_>{
    fn with<T>(&self,operation:impl FnOnce()->Result<T>)->Result<T>{let _identity=self.credentials.map(NetworkIdentity::new).transpose()?;operation()}
    fn open(&self,path:&Path)->Result<fs::File>{self.with(||win::io(fs::OpenOptions::new().read(true).share_mode(FILE_SHARE_READ).open(path)))}
}
struct File{relative:PathBuf,source:PathBuf,metadata:fs::Metadata}
struct Directory{relative:PathBuf,metadata:fs::Metadata}
#[derive(Default)]struct Scan{directories:Vec<Directory>,files:Vec<File>,warnings:Vec<String>,bytes:u64}
fn matches(pattern:&str,value:&str)->bool{
    let pattern=pattern.to_lowercase().chars().collect::<Vec<_>>();let value=value.to_lowercase().chars().collect::<Vec<_>>();
    let (mut p,mut v,mut star,mut retry)=(0,0,None,0);
    while v<value.len(){
        if p<pattern.len()&&(pattern[p]=='?'||pattern[p]==value[v]){p+=1;v+=1;}
        else if p<pattern.len()&&pattern[p]=='*'{star=Some(p);p+=1;retry=v;}
        else if let Some(index)=star{retry+=1;v=retry;p=index+1;}else{return false;}
    }
    while p<pattern.len()&&pattern[p]=='*'{p+=1;}p==pattern.len()
}
fn matches_any(patterns:&str,relative:&Path)->bool{
    let full=relative.to_string_lossy();let name=relative.file_name().unwrap_or_default().to_string_lossy();
    patterns.split([';','\r','\n']).map(str::trim).filter(|s|!s.is_empty()).any(|pattern|matches(pattern,&full)||matches(pattern,&name))
}
fn scan(source:&Source<'_>,options:&Options,cancel:&AtomicBool)->Result<Scan>{
    let mut result=Scan::default();let mut pending=vec![(source.root.clone(),PathBuf::new())];let mut names=std::collections::HashSet::new();
    while let Some((path,relative))=pending.pop(){
        if cancel.load(Ordering::Relaxed){return Err("已取消扫描。".into());}
        let metadata=source.with(||win::io(fs::symlink_metadata(&path)))?;
        if metadata.file_attributes()&FILE_ATTRIBUTE_REPARSE_POINT!=0{result.warnings.push(format!("跳过重解析点：{}",path.display()));continue;}
        if metadata.is_dir(){
            if !relative.as_os_str().is_empty()&&matches_any(&options.exclude_directories,&relative){continue;}
            result.directories.push(Directory{relative:relative.clone(),metadata});
            let entries=source.with(||win::io(fs::read_dir(&path))?.map(|entry|win::io(entry).map(|entry|(entry.path(),entry.file_name()))).collect::<Result<Vec<_>>>())?;
            for (path,name) in entries{pending.push((path,relative.join(name)));}
        }else if metadata.is_file(){
            let relative=if relative.as_os_str().is_empty(){PathBuf::from(path.file_name().ok_or("分发文件名无效。")?)}else{relative};
            if (!options.include_files.trim().is_empty()&&!matches_any(&options.include_files,&relative))||matches_any(&options.exclude_files,&relative)||options.maximum_file_bytes.is_some_and(|maximum|metadata.len()>maximum){continue;}
            if !names.insert(relative.to_string_lossy().to_lowercase()){return Err("源中存在仅大小写不同的重名文件。".into());}
            result.bytes=result.bytes.checked_add(metadata.len()).ok_or("文件总大小溢出。")?;result.files.push(File{relative,source:path,metadata});
        }
    }
    result.directories.sort_by_key(|directory|directory.relative.components().count());Ok(result)
}
fn hash(file:&mut fs::File,cancel:&AtomicBool)->Result<String>{
    win::io(file.seek(SeekFrom::Start(0)))?;let mut hash=Sha256::new();let mut buffer=vec![0u8;1024*1024];
    loop{if cancel.load(Ordering::Relaxed){return Err("已取消校验。".into());}let read=win::io(file.read(&mut buffer))?;if read==0{break;}hash.update(&buffer[..read]);}
    win::io(file.seek(SeekFrom::Start(0)))?;Ok(format!("{:x}",hash.finalize()))
}
fn safe_directory(path:&Path)->Result<()> {
    for ancestor in path.ancestors(){match fs::symlink_metadata(ancestor){
        Ok(metadata) if metadata.file_attributes()&FILE_ATTRIBUTE_REPARSE_POINT!=0||!metadata.is_dir()=>return Err(format!("目标不是普通目录：{}",ancestor.display())),
        Ok(_)=>(),Err(error) if error.kind()==std::io::ErrorKind::NotFound=>(),Err(error)=>return Err(error.to_string())
    }}win::io(fs::create_dir_all(path))
}
static PROMPTS:Mutex<()>=Mutex::new(());
fn directory_metadata(path:&Path,metadata:&fs::Metadata)->Result<()>{
    safe_directory(path)?;
    let file=win::io(fs::OpenOptions::new().write(true).share_mode(FILE_SHARE_READ|FILE_SHARE_WRITE|FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS|FILE_FLAG_OPEN_REPARSE_POINT).open(path))?;
    if win::io(file.metadata())?.file_attributes()&FILE_ATTRIBUTE_REPARSE_POINT!=0{return Err("目标目录已变为重解析点。".into());}
    let time=|value:u64|FILETIME{dwLowDateTime:value as u32,dwHighDateTime:(value>>32) as u32};
    unsafe{win::check(SetFileTime(file.as_raw_handle(),&time(metadata.creation_time()),std::ptr::null(),&time(metadata.last_write_time())),"保存目录时间失败")?;}
    let attributes=metadata.file_attributes()&(FILE_ATTRIBUTE_READONLY|FILE_ATTRIBUTE_HIDDEN|FILE_ATTRIBUTE_SYSTEM|FILE_ATTRIBUTE_ARCHIVE|FILE_ATTRIBUTE_NOT_CONTENT_INDEXED);
    unsafe{win::check(SetFileAttributesW(wide(&path.to_string_lossy()).as_ptr(),attributes|FILE_ATTRIBUTE_DIRECTORY),"保存目录属性失败")}
}
fn confirm(detail:&str,kind:u8,cancel:&AtomicBool)->Result<i32>{
    use windows_sys::Win32::UI::{Controls::*,WindowsAndMessaging::*};
    let _lock=PROMPTS.lock().map_err(|_|"冲突提示状态异常。")?;
    if cancel.load(Ordering::Relaxed){return Err("已取消分发。".into());}
    unsafe extern "system" fn callback(window:windows_sys::Win32::Foundation::HWND,notification:u32,_:usize,_:isize,data:isize)->i32{
        if notification==TDN_TIMER as u32&&unsafe{&*(data as *const AtomicBool)}.load(Ordering::Relaxed){unsafe{PostMessageW(window,TDM_CLICK_BUTTON as u32,IDCANCEL as usize,0);}}0
    }
    let labels=if kind==1{vec![(100,"覆盖此文件"),(101,"跳过此文件"),(102,"覆盖后续全部"),(103,"跳过后续全部")]}else{vec![(100,"确认并继续")]};
    let texts=labels.iter().map(|(_,text)|wide(text)).collect::<Vec<_>>();
    let buttons=labels.iter().zip(&texts).map(|((id,_),text)|TASKDIALOG_BUTTON{nButtonID:*id,pszButtonText:text.as_ptr()}).collect::<Vec<_>>();
    let radio_texts=["覆盖全部","跳过全部","仅覆盖 SHA-256 不同","仅覆盖源文件较新"].map(wide);
    let radios=radio_texts.iter().enumerate().map(|(index,text)|TASKDIALOG_BUTTON{nButtonID:110+index as i32,pszButtonText:text.as_ptr()}).collect::<Vec<_>>();
    let title=wide("文件夹分发");let detail=wide(detail);
    let config=TASKDIALOGCONFIG{cbSize:std::mem::size_of::<TASKDIALOGCONFIG>() as u32,dwFlags:TDF_ALLOW_DIALOG_CANCELLATION|TDF_CALLBACK_TIMER,
        dwCommonButtons:TDCBF_CANCEL_BUTTON,pszWindowTitle:title.as_ptr(),pszContent:detail.as_ptr(),cButtons:buttons.len() as u32,pButtons:buttons.as_ptr(),
        nDefaultButton:IDCANCEL,cRadioButtons:if kind==2{radios.len() as u32}else{0},pRadioButtons:if kind==2{radios.as_ptr()}else{std::ptr::null()},nDefaultRadioButton:112,
        pfCallback:Some(callback),lpCallbackData:cancel as *const AtomicBool as isize,..Default::default()};
    let mut selected=0;let mut radio=112;let code=unsafe{TaskDialogIndirect(&config,&mut selected,&mut radio,std::ptr::null_mut())};
    if code<0{return Err(format!("打开分发确认窗口失败：{code:#x}"));}
    if selected==IDCANCEL||cancel.load(Ordering::Relaxed){cancel.store(true,Ordering::Relaxed);return Err("已取消分发。".into());}Ok(if kind==2{radio}else{selected})
}
fn copy_one(source:&Source<'_>,item:&File,destination:&Path,policy:&Mutex<ConflictPolicy>,options:&Options,cancel:&AtomicBool)->Result<(bool,u64)>{
    if cancel.load(Ordering::Relaxed){return Err("已取消分发。".into());}
    let mut input=source.open(&item.source)?;let current=win::io(input.metadata())?;
    if current.len()!=item.metadata.len()||current.last_write_time()!=item.metadata.last_write_time(){return Err("源文件在扫描后发生变化，请重新扫描。".into());}
    let verify=options.full_verification||item.source.extension().and_then(|ext|ext.to_str()).is_some_and(|ext|EXECUTABLES.iter().any(|value|ext.eq_ignore_ascii_case(value)));
    let original=match fs::symlink_metadata(destination){
        Ok(metadata) if metadata.is_file()&&metadata.file_attributes()&FILE_ATTRIBUTE_REPARSE_POINT==0=>Some(metadata),
        Ok(_)=>return Err("目标不是普通文件。".into()),Err(error) if error.kind()==std::io::ErrorKind::NotFound=>None,Err(error)=>return Err(error.to_string()),
    };
    if let Some(target)=&original{
        let mut policy=policy.lock().map_err(|_|"冲突策略状态异常。")?;
        let overwrite=match *policy{
            ConflictPolicy::Stop=>return Err("目标文件已存在。".into()),ConflictPolicy::Overwrite=>true,ConflictPolicy::Skip=>false,
            ConflictPolicy::Newer=>win::io(current.modified())?>win::io(target.modified())?+Duration::from_secs(1),
            ConflictPolicy::DifferentHash=>current.len()!=target.len()||hash(&mut input,cancel)?!=hash(&mut win::io(fs::File::open(destination))?,cancel)?,
            ConflictPolicy::Ask=>{
                let source_hash=hash(&mut input,cancel)?;let target_hash=hash(&mut win::io(fs::File::open(destination))?,cancel)?;
                match confirm(&format!("{}\n源：{} 字节\n目标：{} 字节\n源 SHA-256：{source_hash}\n目标 SHA-256：{target_hash}",destination.display(),current.len(),target.len()),1,cancel)?{
                    100=>true,101=>false,102=>{*policy=ConflictPolicy::Overwrite;true},103=>{*policy=ConflictPolicy::Skip;false},_=>return Err("冲突处理选择无效。".into())
                }
            }
        };if !overwrite{return Ok((false,0));}
    }
    safe_directory(destination.parent().ok_or("目标文件缺少目录。")?)?;
    let temporary=destination.with_file_name(format!(".corprust-{}.partial",model::new_id()));
    struct Temporary(PathBuf);impl Drop for Temporary{fn drop(&mut self){let _=fs::remove_file(&self.0);}}
    let _temporary=Temporary(temporary.clone());
    let mut output=win::io(fs::OpenOptions::new().write(true).create_new(true).share_mode(0).open(&temporary))?;
    let mut digest=Sha256::new();let mut buffer=vec![0u8;1024*1024];let mut copied=0u64;
    loop{if cancel.load(Ordering::Relaxed){return Err("已取消，未完成的临时文件将被清理。".into());}let read=win::io(input.read(&mut buffer))?;if read==0{break;}win::io(output.write_all(&buffer[..read]))?;copied+=read as u64;if verify{digest.update(&buffer[..read]);}}
    if copied!=current.len(){return Err("复制字节数与源文件大小不一致。".into());}
    win::io(output.sync_all())?;
    let time=|value:u64|FILETIME{dwLowDateTime:value as u32,dwHighDateTime:(value>>32) as u32};
    unsafe{win::check(SetFileTime(output.as_raw_handle(),&time(current.creation_time()),std::ptr::null(),&time(current.last_write_time())),"保存文件时间失败")?;}drop(output);
    if verify&&format!("{:x}",digest.finalize())!=hash(&mut win::io(fs::File::open(&temporary))?,cancel)?{return Err("目标临时文件 SHA-256 不匹配，原文件已保留。".into());}
    if cancel.load(Ordering::Relaxed){return Err("已取消分发。".into());}
    if let Some(metadata)=&original{if metadata.file_attributes()&FILE_ATTRIBUTE_READONLY!=0{unsafe{win::check(SetFileAttributesW(wide(&destination.to_string_lossy()).as_ptr(),metadata.file_attributes()&!FILE_ATTRIBUTE_READONLY),"无法解除旧文件只读属性")?;}}}
    if unsafe{MoveFileExW(wide(&temporary.to_string_lossy()).as_ptr(),wide(&destination.to_string_lossy()).as_ptr(),MOVEFILE_WRITE_THROUGH|if original.is_some(){MOVEFILE_REPLACE_EXISTING}else{0})}==0{
        let error=win::error("提交分发文件失败");if let Some(metadata)=&original{unsafe{SetFileAttributesW(wide(&destination.to_string_lossy()).as_ptr(),metadata.file_attributes());}}return Err(error);
    }
    let attributes=current.file_attributes()&(FILE_ATTRIBUTE_READONLY|FILE_ATTRIBUTE_HIDDEN|FILE_ATTRIBUTE_SYSTEM|FILE_ATTRIBUTE_ARCHIVE|FILE_ATTRIBUTE_NOT_CONTENT_INDEXED);
    unsafe{win::check(SetFileAttributesW(wide(&destination.to_string_lossy()).as_ptr(),attributes|FILE_ATTRIBUTE_ARCHIVE),"文件已复制，但保存属性失败")?;}
    Ok((true,copied))
}

pub fn run(computer:&str,target_credentials:&Credentials,source_credentials:Option<&Credentials>,request:&Request,report:&Reporter,cancel:&AtomicBool,progress:Option<&(dyn Fn(Value)+Sync)>)->Result<Value>{
    let options=&request.distribution;
    let source=Source{root:if options.source_computer.is_empty(){PathBuf::from(&request.source)}else{PathBuf::from(model::unc(&options.source_computer,&request.source)?)},credentials:source_credentials};
    let target=if options.destination_preset=="Custom"{request.destination.clone()}else{
        let mut resolve=request.clone();resolve.id=model::new_id();resolve.operation="resolve-folders".into();
        crate::remote::execute(computer,target_credentials,&resolve,report,cancel)?["destination"].as_str().ok_or("目标预设未返回路径。")?.to_owned()
    };
    let destination=PathBuf::from(model::unc(computer,&target)?);
    if !options.source_computer.is_empty(){
        use std::net::ToSocketAddrs;
        let source_addresses=(options.source_computer.as_str(),445).to_socket_addrs().ok().into_iter().flatten().map(|a|a.ip()).collect::<Vec<_>>();
        let same_host=options.source_computer.eq_ignore_ascii_case(computer)||(computer,445).to_socket_addrs().ok().into_iter().flatten().any(|a|source_addresses.contains(&a.ip()));
        let src=request.source.trim_end_matches('\\').to_lowercase();let dest=target.trim_end_matches('\\').to_lowercase();
        if same_host&&(src==dest||dest.starts_with(&format!("{src}\\"))){return Err("目标不能是源目录自身或其子目录。".into());}
    }else{
        // File identities detect local paths and UNC aliases that resolve to the same directory.
        let source_canonical=source.with(||win::io(fs::canonicalize(&source.root)))?;
        for ancestor in destination.ancestors(){if fs::canonicalize(ancestor).ok().is_some_and(|path|path==source_canonical){return Err("目标位于源目录之内。".into());}}
    }
    report(format!("{computer}：扫描分发源…"));let scan=scan(&source,options,cancel)?;
    if scan.files.len()>100_000||scan.bytes>20*1024*1024*1024{confirm(&format!("{} 个文件，共 {:.2} GiB，确认继续？",scan.files.len(),scan.bytes as f64/1073741824.0),0,cancel)?;}
    for directory in &scan.directories{if cancel.load(Ordering::Relaxed){return Err("已取消分发。".into());}safe_directory(&destination.join(&directory.relative))?;}
    let directory=crate::tasks::data_directory()?.join("DistributionReports");win::io(fs::create_dir_all(&directory))?;
    let report_path=directory.join(format!("{}-{computer}.jsonl",request.id));let output=Mutex::new(win::io(fs::OpenOptions::new().write(true).create_new(true).open(&report_path))?);
    let mut effective_policy=request.conflict;
    if matches!(effective_policy,ConflictPolicy::Ask){
        let mut conflicts=0;
        for item in &scan.files{
            if cancel.load(Ordering::Relaxed){return Err("已取消分发。".into());}
            if destination.join(&item.relative).is_file(){conflicts+=1;}
            if conflicts>1000{
                effective_policy=match confirm(&format!("{computer} 至少存在 {conflicts} 个同名文件。为避免连续弹出大量确认窗口，请选择本机统一处理策略。"),2,cancel)?{
                    110=>ConflictPolicy::Overwrite,111=>ConflictPolicy::Skip,112=>ConflictPolicy::DifferentHash,113=>ConflictPolicy::Newer,_=>return Err("大量同名文件处理策略无效。".into())
                };break;
            }
        }
    }
    let policy=Mutex::new(effective_policy);let next=AtomicUsize::new(0);let copied=AtomicUsize::new(0);let skipped=AtomicUsize::new(0);let failed=AtomicUsize::new(0);
    let bytes=std::sync::atomic::AtomicU64::new(0);let errors=Mutex::new(Vec::<String>::new());let last_report=Mutex::new(std::time::Instant::now());
    std::thread::scope(|scope|{for _ in 0..options.file_concurrency.min(scan.files.len()){
        scope.spawn(||{
            let identity=NetworkIdentity::new(target_credentials);
            loop{
                if cancel.load(Ordering::Relaxed){break;}
                let index=next.fetch_add(1,Ordering::Relaxed);let Some(file)=scan.files.get(index)else{break;};
                let result=identity.as_ref().map_err(|e|e.clone()).and_then(|_|copy_one(&source,file,&destination.join(&file.relative),&policy,options,cancel));
                let row=match result{
                    Ok((true,count))=>{copied.fetch_add(1,Ordering::Relaxed);bytes.fetch_add(count,Ordering::Relaxed);json!({"path":file.relative,"action":"Copied","bytes":count})},
                    Ok((false,_))=>{skipped.fetch_add(1,Ordering::Relaxed);json!({"path":file.relative,"action":"Skipped"})},
                    Err(error)=>{failed.fetch_add(1,Ordering::Relaxed);let mut errors=errors.lock().unwrap();if errors.len()<200{errors.push(format!("{}：{error}",file.relative.display()));}json!({"path":file.relative,"action":"Failed","error":error})}
                };
                if let Err(error)=output.lock().map_err(|_|"报告状态异常。".to_string()).and_then(|mut file|win::io(writeln!(file,"{row}"))){cancel.store(true,Ordering::Relaxed);errors.lock().unwrap().push(error);break;}
                let mut last=last_report.lock().unwrap();if last.elapsed()>=Duration::from_millis(150){
                    *last=std::time::Instant::now();
                    let completed=copied.load(Ordering::Relaxed)+skipped.load(Ordering::Relaxed)+failed.load(Ordering::Relaxed);
                    let percent=if scan.files.is_empty(){100}else{completed*100/scan.files.len()};
                    let detail=format!("{completed}/{}，复制 {}，跳过 {}，失败 {}",scan.files.len(),copied.load(Ordering::Relaxed),skipped.load(Ordering::Relaxed),failed.load(Ordering::Relaxed));
                    report(format!("{computer}：{detail}"));
                    if let Some(progress)=progress{progress(json!({"progress":percent,"files":copied.load(Ordering::Relaxed),"skipped":skipped.load(Ordering::Relaxed),"failed":failed.load(Ordering::Relaxed),"detail":detail}));}
                }
            }
        });
    }});
    for directory in scan.directories.iter().rev(){
        if cancel.load(Ordering::Relaxed){break;}
        if let Err(error)=directory_metadata(&destination.join(&directory.relative),&directory.metadata){
            failed.fetch_add(1,Ordering::Relaxed);errors.lock().unwrap().push(error.clone());
            let row=json!({"relativePath":directory.relative,"state":"SetDirectoryMetadataFailed","bytes":0,"detail":error});
            if let Err(error)=output.lock().map_err(|_|"报告状态异常。".to_string()).and_then(|mut file|win::io(writeln!(file,"{row}"))){errors.lock().unwrap().push(error);}
        }
    }
    let copied=copied.into_inner();let skipped=skipped.into_inner();let failed=failed.into_inner();let cancelled=cancel.load(Ordering::Relaxed);let succeeded=failed==0&&!cancelled;
    Ok(json!({"succeeded":succeeded,"partialSuccess":!succeeded&&copied>0,"cancelled":cancelled,"files":copied,"skipped":skipped,"failed":failed,"bytes":bytes.into_inner(),
        "totalFiles":scan.files.len(),"destination":target,"reportPath":report_path,"warnings":scan.warnings,"errors":errors.into_inner().unwrap(),
        "error":if cancelled{Some("已取消剩余分发任务。".to_string())}else if failed>0{Some(format!("{failed} 个文件失败，请查看逐文件报告。"))}else{None}}))
}
pub fn resolve(request:&Request)->Result<Value>{
    let preset=request.distribution.destination_preset.as_str();let data=std::env::var("ProgramData").unwrap_or("C:\\ProgramData".into());
    let path=match preset{
        "PublicDesktop"=>win::common_folder(&windows_sys::Win32::UI::Shell::FOLDERID_PublicDesktop)?,"PublicStartMenu"=>win::common_folder(&windows_sys::Win32::UI::Shell::FOLDERID_CommonPrograms)?,"ProgramDataCompany"=>PathBuf::from(data).join("Company"),
        "UserDesktop"|"UserDocuments"|"UserDownloads"=>{
            let sessions=win::sessions(None)?;let users=sessions.as_array().into_iter().flatten().filter(|row|row["state"]=="Active"&&(request.session==0||row["sessionId"]==request.session)).collect::<Vec<_>>();
            let account=if !request.distribution.user_account.is_empty(){request.distribution.user_account.as_str()}else{match users.as_slice(){[user]=>user["account"].as_str().ok_or("登录用户未知。")?,_=>return Err("用户目录预设需要明确选择目标电脑的登录用户。".into())}};
            let (name,fallback)=match preset{"UserDesktop"=>("Desktop","Desktop"),"UserDocuments"=>("Personal","Documents"),_=>("{374DE290-123F-4565-9164-39C4925E467B}","Downloads")};
            win::profile_folder(account,name,fallback)?
        },_=>return Err("目标预设无效。".into())
    };model::drive_path(&path.to_string_lossy())?;Ok(json!({"destination":path}))
}

#[cfg(test)]mod tests{
    use super::*;
    struct Fixture(PathBuf);
    impl Fixture{fn new()->Self{let path=std::env::temp_dir().join(format!("CorpRustDistributionTest-{}",model::new_id()));fs::create_dir(&path).unwrap();Self(path)}}
    impl Drop for Fixture{fn drop(&mut self){if self.0.parent()==Some(std::env::temp_dir().as_path())&&self.0.file_name().is_some_and(|name|name.to_string_lossy().starts_with("CorpRustDistributionTest-")){let _=fs::remove_dir_all(&self.0);}}}
    #[test]fn directory_copy_preserves_source_timestamps_and_attributes(){
        let fixture=Fixture::new();let source=fixture.0.join("source");let target=fixture.0.join("target");fs::create_dir(&source).unwrap();fs::create_dir(&target).unwrap();
        let directory=fs::OpenOptions::new().write(true).custom_flags(FILE_FLAG_BACKUP_SEMANTICS).open(&source).unwrap();
        let ticks=132_537_600_000_000_000u64;let time=FILETIME{dwLowDateTime:ticks as u32,dwHighDateTime:(ticks>>32) as u32};
        unsafe{assert_ne!(SetFileTime(directory.as_raw_handle(),&time,std::ptr::null(),&time),0);assert_ne!(SetFileAttributesW(wide(&source.to_string_lossy()).as_ptr(),FILE_ATTRIBUTE_HIDDEN|FILE_ATTRIBUTE_DIRECTORY),0);}drop(directory);
        let expected=fs::metadata(&source).unwrap();directory_metadata(&target,&expected).unwrap();let actual=fs::metadata(&target).unwrap();
        assert_eq!(actual.creation_time(),expected.creation_time());assert_eq!(actual.last_write_time(),expected.last_write_time());assert_ne!(actual.file_attributes()&FILE_ATTRIBUTE_HIDDEN,0);
    }
    #[test]fn atomic_copy_verifies_equal_length_changes_and_preserves_cancelled_targets(){
        let fixture=Fixture::new();let source_path=fixture.0.join("source.exe");let destination=fixture.0.join("target.exe");
        fs::write(&source_path,b"new bytes").unwrap();fs::write(&destination,b"old bytes").unwrap();
        let source=Source{root:source_path.clone(),credentials:None};let item=File{relative:"source.exe".into(),source:source_path.clone(),metadata:fs::metadata(&source_path).unwrap()};let cancel=AtomicBool::new(false);let policy=Mutex::new(ConflictPolicy::DifferentHash);
        assert_eq!(copy_one(&source,&item,&destination,&policy,&Options::default(),&cancel).unwrap(),(true,9));
        assert_eq!(fs::read(&destination).unwrap(),b"new bytes");
        assert_eq!(copy_one(&source,&item,&destination,&policy,&Options::default(),&cancel).unwrap(),(false,0));
        fs::write(&source_path,b"changed!!").unwrap();let item=File{metadata:fs::metadata(&source_path).unwrap(),..item};cancel.store(true,Ordering::Relaxed);
        assert!(copy_one(&source,&item,&destination,&policy,&Options::default(),&cancel).is_err());assert_eq!(fs::read(&destination).unwrap(),b"new bytes");
        assert!(!fs::read_dir(&fixture.0).unwrap().flatten().any(|entry|entry.file_name().to_string_lossy().ends_with(".partial")));
    }
    #[test]fn changed_source_is_rejected_before_replacing_destination(){
        let fixture=Fixture::new();let path=fixture.0.join("source.txt");let destination=fixture.0.join("target.txt");fs::write(&path,b"before").unwrap();fs::write(&destination,b"keep me").unwrap();
        let item=File{relative:"source.txt".into(),source:path.clone(),metadata:fs::metadata(&path).unwrap()};fs::write(&path,b"changed length").unwrap();
        let source=Source{root:path,credentials:None};assert!(copy_one(&source,&item,&destination,&Mutex::new(ConflictPolicy::Overwrite),&Options::default(),&AtomicBool::new(false)).is_err());assert_eq!(fs::read(&destination).unwrap(),b"keep me");
    }
    #[test]fn filters_and_high_risk_paths(){
        assert!(matches("*.PDF","Report.pdf"));assert!(matches("~$*","~$draft.docx"));assert!(!matches("*.pdf","report.pdf.exe"));assert!(matches_any("Temp;Cache;node_modules",Path::new("a\\Cache")));
        assert!(high_risk("D:\\Windows\\System32"));assert!(high_risk("C:\\Users\\Alice\\Desktop"));assert!(!high_risk("D:\\CompanyFiles"));
    }
}

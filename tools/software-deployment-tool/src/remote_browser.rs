use crate::{model::{self,Credentials,Result},win::{self,wide,NetworkIdentity}};
use std::{fs,mem::zeroed,path::Path,ptr::{null,null_mut},sync::{Arc,mpsc,atomic::{AtomicBool,Ordering}}};
use std::os::windows::fs::MetadataExt;
use windows_sys::Win32::{Foundation::*,Graphics::Gdi::*,Storage::FileSystem::*,NetworkManagement::NetManagement::NetApiBufferFree,
    System::LibraryLoader::*,UI::{Controls::*,Input::KeyboardAndMouse::EnableWindow,WindowsAndMessaging::*}};

#[derive(Clone,Copy)]pub enum Mode{Installer,Executable,Elevation,AnyFile,Folder}
impl Mode{fn accepts(self,path:&str)->bool{let extension=Path::new(path).extension().and_then(|s|s.to_str()).unwrap_or_default();match self{
    Self::Installer=>["exe","msi"].iter().any(|value|extension.eq_ignore_ascii_case(value)),Self::Executable=>extension.eq_ignore_ascii_case("exe"),
    Self::Elevation=>["exe","lnk"].iter().any(|value|extension.eq_ignore_ascii_case(value)),Self::AnyFile=>true,Self::Folder=>false
}}}
struct Entry{path:String,name:String,directory:bool,size:u64,modified:String,public_desktop:bool}
pub(crate) fn suggestions_mode(host:&str,credentials:&Credentials,query:&str,mode:Mode)->Result<Vec<(String,bool)>>{
    let cancel=AtomicBool::new(false);
    let query=query.trim().trim_matches('"');let query=if query.len()==2&&query.as_bytes()[1]==b':'{format!("{query}\\")}else{query.to_owned()};
    if query.is_empty(){return list(host,credentials,"",mode,&cancel).map(|entries|entries.into_iter().map(|entry|(entry.path,entry.directory)).collect());}
    model::drive_path(&query)?;
    match list(host,credentials,&query,mode,&cancel){
        Ok(entries)=>Ok(entries.into_iter().map(|entry|(entry.path,entry.directory)).take(500).collect()),
        Err(original)=>{
            let path=Path::new(&query);let Some(parent)=path.parent()else{return Err(original);};
            let prefix=path.file_name().unwrap_or_default().to_string_lossy().to_lowercase();
            list(host,credentials,&parent.to_string_lossy(),mode,&cancel).map(|entries|entries.into_iter().filter(|entry|entry.name.to_lowercase().starts_with(&prefix)).map(|entry|(entry.path,entry.directory)).take(500).collect())
        }
    }
}
fn time(ticks:u64)->String{unsafe{
    use windows_sys::Win32::System::Time::FileTimeToSystemTime;
    let original=FILETIME{dwLowDateTime:ticks as u32,dwHighDateTime:(ticks>>32) as u32};let mut local=FILETIME::default();let mut time=SYSTEMTIME::default();
    if FileTimeToLocalFileTime(&original,&mut local)==0||FileTimeToSystemTime(&local,&mut time)==0{return String::new();}
    format!("{:04}-{:02}-{:02} {:02}:{:02}",time.wYear,time.wMonth,time.wDay,time.wHour,time.wMinute)
}}
fn list(host:&str,credentials:&Credentials,path:&str,mode:Mode,cancel:&AtomicBool)->Result<Vec<Entry>>{
    let _identity=NetworkIdentity::new(credentials)?;let mut entries=Vec::new();
    if path.is_empty(){unsafe{
        let mut resume=0;
        loop{
            if cancel.load(Ordering::Relaxed){return Err("已取消浏览。".into());}
            let mut buffer=null_mut();let mut count=0;let mut total=0;
            let code=NetShareEnum(wide(&format!("\\\\{host}")).as_ptr(),1,&mut buffer,u32::MAX,&mut count,&mut total,&mut resume);
            struct Buffer(*mut u8);impl Drop for Buffer{fn drop(&mut self){if !self.0.is_null(){unsafe{NetApiBufferFree(self.0.cast());}}}}
            let _buffer=Buffer(buffer);
            if code!=ERROR_SUCCESS&&code!=ERROR_MORE_DATA{return Err(format!("读取目标机磁盘失败：{}",std::io::Error::from_raw_os_error(code as i32)));}
            if count>10000||count>0&&buffer.is_null(){return Err("目标机共享列表无效。".into());}
            for share in std::slice::from_raw_parts(if buffer.is_null(){std::ptr::NonNull::<SHARE_INFO_1>::dangling().as_ptr()}else{buffer.cast::<SHARE_INFO_1>()},count as usize){
                let name=win::string(share.shi1_netname);let bytes=name.as_bytes();
                if bytes.len()==2&&bytes[0].is_ascii_alphabetic()&&bytes[1]==b'$'{let drive=format!("{}:\\",(bytes[0] as char).to_ascii_uppercase());entries.push(Entry{path:drive.clone(),name:drive,directory:true,size:0,modified:String::new(),public_desktop:false});}
            }
            if code==ERROR_SUCCESS{break;}
        }
    }}else{
        model::drive_path(path)?;
        for entry in win::io(fs::read_dir(model::unc(host,path)?))?{
            if cancel.load(Ordering::Relaxed){return Err("已取消浏览。".into());}
            let entry=win::io(entry)?;let metadata=match fs::symlink_metadata(entry.path()){Ok(value)=>value,Err(_)=>continue};
            if metadata.file_attributes()&FILE_ATTRIBUTE_REPARSE_POINT!=0{continue;}
            let name=entry.file_name().to_string_lossy().into_owned();if !metadata.is_dir()&&!mode.accepts(&name){continue;}
            entries.push(Entry{path:format!("{}\\{name}",path.trim_end_matches('\\')),name,directory:metadata.is_dir(),size:metadata.len(),modified:time(metadata.last_write_time()),public_desktop:false});
        }
    }
    entries.sort_by_key(|entry|(!entry.directory,entry.name.to_lowercase()));Ok(entries)
}

// Explorer combines the standard per-user Desktop with the sibling Public Desktop.
// Restrict the merge to that standard layout rather than guessing redirected paths.
fn public_desktop_for(path: &str) -> Option<std::path::PathBuf> {
    let desktop = Path::new(path.trim_end_matches('\\'));
    if !desktop.file_name()?.eq_ignore_ascii_case("Desktop") { return None; }
    let profile = desktop.parent()?;
    if profile.file_name()?.eq_ignore_ascii_case("Public") { return None; }
    let users = profile.parent()?;
    if !users.file_name()?.eq_ignore_ascii_case("Users") || users.parent()?.parent().is_some() { return None; }
    Some(users.join("Public").join("Desktop"))
}

struct Listing {
    entries: Vec<Entry>,
    note: String,
}

#[cfg(test)]
#[test]
fn desktop_merge_preserves_the_target_volume_and_excludes_non_desktop_folders() {
    assert_eq!(public_desktop_for(r"D:\Users\TEST\Desktop").as_deref(), Some(Path::new(r"D:\Users\Public\Desktop")));
    for path in [r"C:\Users\Public\Desktop", r"C:\Archive\Users\TEST\Desktop", r"C:\Users\TEST\Downloads"] {
        assert!(public_desktop_for(path).is_none(), "{path}");
    }
}

fn browse_listing(host: &str, credentials: &Credentials, path: &str, mode: Mode, cancel: &AtomicBool) -> Result<Listing> {
    let mut entries = list(host, credentials, path, mode, cancel)?;
    let mut note = String::new();
    if let Some(public) = matches!(mode, Mode::Elevation).then(|| public_desktop_for(path)).flatten() {
        match list(host, credentials, &public.to_string_lossy(), mode, cancel) {
            Ok(public_entries) => {
                let count = public_entries.iter().filter(|entry| !entry.directory).count();
                entries.extend(public_entries.into_iter().filter(|entry| !entry.directory).map(|mut entry| {
                    entry.public_desktop = true;
                    entry
                }));
                note = format!("（公共桌面 {count} 项）");
            }
            Err(error) => note = format!("（公共桌面读取失败：{error}）"),
        }
    }
    Ok(Listing { entries, note })
}

struct State{
    host:String,credentials:Credentials,mode:Mode,path:String,entries:Vec<Entry>,result:Option<String>,done:bool,loading:bool,sequence:u64,
    sender:mpsc::Sender<(u64,Result<Listing>)>,receiver:mpsc::Receiver<(u64,Result<Listing>)>,cancel:Arc<AtomicBool>,font:HFONT,
}
unsafe fn set(window:HWND,id:i32,text:&str){unsafe{SetWindowTextW(GetDlgItem(window,id),wide(text).as_ptr());}}
unsafe fn text(window:HWND,id:i32)->String{unsafe{let control=GetDlgItem(window,id);let mut data=vec![0u16;GetWindowTextLengthW(control) as usize+1];GetWindowTextW(control,data.as_mut_ptr(),data.len() as i32);String::from_utf16_lossy(&data[..data.len()-1])}}
unsafe fn navigate(window:HWND,state:&mut State,path:String){unsafe{
    if state.loading{return;}
    if !path.is_empty(){if let Err(error)=model::drive_path(&path){set(window,12,&error);return;}}
    state.path=path.clone();state.sequence+=1;state.loading=true;state.entries.clear();
    SendMessageW(GetDlgItem(window,11),LVM_DELETEALLITEMS,0,0);set(window,10,&path);set(window,12,"正在读取目标机文件，请稍候…");
    for id in [1,3,4,5,10,11]{EnableWindow(GetDlgItem(window,id),0);}
    let host=state.host.clone();let credentials=state.credentials.clone();let mode=state.mode;let cancel=state.cancel.clone();let sender=state.sender.clone();let sequence=state.sequence;
    std::thread::spawn(move||{let result=browse_listing(&host,&credentials,&path,mode,&cancel);let _=sender.send((sequence,result));});
}}
unsafe fn choose(window:HWND,state:&mut State){unsafe{
    if state.loading{return;}
    let path=text(window,10).trim().trim_matches('"').to_owned();if path!=state.path{navigate(window,state,path);return;}
    let selected=SendMessageW(GetDlgItem(window,11),LVM_GETNEXTITEM,usize::MAX,LVNI_SELECTED as isize);
    let Some(entry)=state.entries.get(selected as usize)else{set(window,12,"请选择文件或目录；也可在路径栏输入目录后按 Enter。");return;};
    let path=entry.path.clone();if entry.directory{navigate(window,state,path);}else{state.result=Some(path);DestroyWindow(window);}
}}
unsafe extern "system" fn procedure(window:HWND,message:u32,w:WPARAM,l:LPARAM)->LRESULT{unsafe{
    if message==WM_NCCREATE{SetWindowLongPtrW(window,GWLP_USERDATA,(*(l as *const CREATESTRUCTW)).lpCreateParams as isize);}
    let pointer=GetWindowLongPtrW(window,GWLP_USERDATA) as *mut State;if pointer.is_null(){return DefWindowProcW(window,message,w,l);}
    let state=&mut *pointer;
    match message{
        WM_COMMAND=>{match (w&0xffff) as i32{
            1=>choose(window,state),2=>{DestroyWindow(window);},
            3 if !state.loading=>{let parent=Path::new(&state.path).parent().map(|p|p.to_string_lossy().into_owned()).unwrap_or_default();navigate(window,state,parent);},
            4=>{let path=text(window,10).trim().trim_matches('"').to_owned();navigate(window,state,path);},
            5 if !state.loading&&!state.path.is_empty()=>{state.result=Some(state.path.clone());DestroyWindow(window);},_=>()
        }0},
        WM_NOTIFY=>{let notification=&*(l as *const NMHDR);if notification.idFrom==11&&notification.code==NM_DBLCLK{choose(window,state);}0},
        WM_TIMER=>{
            while let Ok((sequence,result))=state.receiver.try_recv(){
                if sequence!=state.sequence{continue;}state.loading=false;
                for id in [1,3,4,5,10,11]{EnableWindow(GetDlgItem(window,id),1);}
                EnableWindow(GetDlgItem(window,5),(!state.path.is_empty()) as i32);
                match result{
                    Err(error)=>set(window,12,&error),
                    Ok(listing)=>{
                        state.entries=listing.entries;let list=GetDlgItem(window,11);SendMessageW(list,WM_SETREDRAW,0,0);
                        for (index,entry) in state.entries.iter().enumerate(){
                            let values=[if entry.directory{"目录".into()}else if entry.public_desktop{"公共桌面".into()}else{"文件".into()},entry.name.clone(),if entry.directory{String::new()}else{format!("{:.1} KiB",entry.size as f64/1024.0)},entry.modified.clone()];
                            for (column,value) in values.iter().enumerate(){let mut value=wide(value);let row=LVITEMW{mask:LVIF_TEXT,iItem:index as i32,iSubItem:column as i32,pszText:value.as_mut_ptr(),..zeroed()};SendMessageW(list,if column==0{LVM_INSERTITEMW}else{LVM_SETITEMW},0,(&row as *const LVITEMW) as isize);}
                        }
                        SendMessageW(list,WM_SETREDRAW,1,0);InvalidateRect(list,null(),1);
                        set(window,12,&format!("{} 个项目{}。双击目录进入，选中文件后点击“选择”。",state.entries.len(),listing.note));
                    }
                }
            }0
        },
        WM_SIZE=>{
            let width=(l&0xffff) as i32;let height=((l>>16)&0xffff) as i32;
            for (id,x,y,w,h) in [(3,16,16,94,34),(10,122,16,width-246,34),(4,width-112,16,96,34),(11,16,64,width-32,height-164),(12,16,height-90,width-32,30),(5,16,height-48,174,34),(2,width-238,height-48,104,34),(1,width-120,height-48,104,34)]{MoveWindow(GetDlgItem(window,id),x,y,w.max(1),h.max(1),1);}0
        },
        WM_CLOSE=>{DestroyWindow(window);0},WM_DESTROY=>{KillTimer(window,1);state.cancel.store(true,Ordering::Relaxed);state.done=true;0},
        WM_NCDESTROY=>{SetWindowLongPtrW(window,GWLP_USERDATA,0);DefWindowProcW(window,message,w,l)},_=>DefWindowProcW(window,message,w,l)
    }
}}
pub fn choose_remote(owner:HWND,host:&str,credentials:Credentials,initial:&str,mode:Mode)->Result<Option<String>>{
    model::host(host)?;
    let path=if model::drive_path(initial).is_ok(){if !Path::new(initial).extension().is_none() && mode.accepts(initial){Path::new(initial).parent().map(|p|p.to_string_lossy().into_owned()).unwrap_or_default()}else{initial.into()}}else{String::new()};
    let (sender,receiver)=mpsc::channel();
    unsafe{
        let instance=GetModuleHandleW(null());let class=wide("CorpRustRemoteBrowser");let wc=WNDCLASSW{lpfnWndProc:Some(procedure),hInstance:instance,hCursor:LoadCursorW(null_mut(),IDC_ARROW),hbrBackground:(COLOR_WINDOW+1) as HBRUSH,lpszClassName:class.as_ptr(),..zeroed()};RegisterClassW(&wc);
        let font=CreateFontW(-15,0,0,0,400,0,0,0,DEFAULT_CHARSET as u32,0,0,CLEARTYPE_QUALITY as u32,0,wide("Microsoft YaHei UI").as_ptr());
        let mut state=Box::new(State{host:host.into(),credentials,mode,path:String::new(),entries:Vec::new(),result:None,done:false,loading:false,sequence:0,sender,receiver,cancel:Arc::new(AtomicBool::new(false)),font});
        let window=CreateWindowExW(WS_EX_DLGMODALFRAME,class.as_ptr(),wide(&format!("浏览目标电脑 — {host}")).as_ptr(),WS_OVERLAPPEDWINDOW,CW_USEDEFAULT,CW_USEDEFAULT,960.min(GetSystemMetrics(SM_CXSCREEN)-40),680.min(GetSystemMetrics(SM_CYSCREEN)-60),owner,null_mut(),instance,(&mut *state as *mut State).cast());
        if window.is_null(){DeleteObject(font);return Err(win::error("创建远程浏览窗口失败"));}
        for (id,class,label,style) in [(1,"BUTTON","选择",WS_TABSTOP|BS_DEFPUSHBUTTON as u32),(2,"BUTTON","取消",WS_TABSTOP),(3,"BUTTON","上一级",WS_TABSTOP),(4,"BUTTON","刷新",WS_TABSTOP),(5,"BUTTON","选择当前文件夹",WS_TABSTOP),(10,"EDIT","",WS_TABSTOP|ES_AUTOHSCROLL as u32),(11,"SysListView32","",WS_TABSTOP|LVS_REPORT|LVS_SINGLESEL|LVS_SHOWSELALWAYS),(12,"STATIC","",0)]{
            let control=CreateWindowExW(if class=="EDIT"{WS_EX_CLIENTEDGE}else{0},wide(class).as_ptr(),wide(label).as_ptr(),WS_CHILD|WS_VISIBLE|style,0,0,1,1,window,id as usize as HMENU,instance,null());SendMessageW(control,WM_SETFONT,font as usize,1);
        }
        if !matches!(mode,Mode::Folder){ShowWindow(GetDlgItem(window,5),SW_HIDE);}
        SendMessageW(GetDlgItem(window,10),EM_SETLIMITTEXT,32768,0);
        let list=GetDlgItem(window,11);windows_sys::Win32::UI::Shell::SetWindowSubclass(list,Some(win::listview_scroll),2,0);SendMessageW(list,LVM_SETEXTENDEDLISTVIEWSTYLE,0,(LVS_EX_FULLROWSELECT|LVS_EX_DOUBLEBUFFER) as isize);
        for (index,(label,width)) in [("类型",80),("名称",430),("大小",130),("修改时间",185)].iter().enumerate(){let mut label=wide(label);let column=LVCOLUMNW{mask:LVCF_TEXT|LVCF_WIDTH,cx:*width,pszText:label.as_mut_ptr(),..zeroed()};SendMessageW(list,LVM_INSERTCOLUMNW,index,(&column as *const LVCOLUMNW) as isize);}
        let mut rect:RECT=zeroed();GetClientRect(window,&mut rect);SendMessageW(window,WM_SIZE,0,((rect.bottom as usize)<<16|rect.right as usize) as isize);SetTimer(window,1,100,None);
        EnableWindow(owner,0);ShowWindow(window,SW_SHOW);SetForegroundWindow(window);navigate(window,&mut state,path);
        let mut message:MSG=zeroed();let mut quit=None;
        while !state.done{let result=GetMessageW(&mut message,null_mut(),0,0);if result<=0{if result==0{quit=Some(message.wParam);}if IsWindow(window)!=0{DestroyWindow(window);}break;}if IsDialogMessageW(window,&message)==0{TranslateMessage(&message);DispatchMessageW(&message);}}
        EnableWindow(owner,1);SetForegroundWindow(owner);DeleteObject(state.font);if let Some(code)=quit{PostQuitMessage(code as i32);}Ok(state.result.take())
    }
}
#[cfg(test)]mod tests{use super::*;#[test]fn browser_modes_keep_folder_and_file_roles_distinct(){assert!(Mode::Installer.accepts("a.MSI"));assert!(!Mode::Installer.accepts("a.lnk"));assert!(Mode::Elevation.accepts("a.LNK"));assert!(!Mode::Folder.accepts("a.exe"));assert!(Mode::AnyFile.accepts("a.dat"));}}

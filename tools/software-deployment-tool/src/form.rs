use crate::{model::Result,win::{self,wide}};
use std::{mem::zeroed,ptr::{null,null_mut}};
use windows_sys::Win32::{Foundation::*,Graphics::Gdi::*,System::LibraryLoader::*,UI::{Controls::*,Input::KeyboardAndMouse::EnableWindow,WindowsAndMessaging::*}};

pub enum Kind{Check(bool),Text(String),Choice(Vec<String>,usize),Memo(String)}
pub struct Field{pub label:String,pub kind:Kind}
impl Field{
    pub fn check(label:&str,value:bool)->Self{Self{label:label.into(),kind:Kind::Check(value)}}
    pub fn text(label:&str,value:&str)->Self{Self{label:label.into(),kind:Kind::Text(value.into())}}
    pub fn choice(label:&str,items:&[&str],selected:usize)->Self{Self{label:label.into(),kind:Kind::Choice(items.iter().map(|s|(*s).into()).collect(),selected)}}
    pub fn memo(value:&str)->Self{Self{label:String::new(),kind:Kind::Memo(value.into())}}
}
struct State{fields:Vec<Field>,controls:Vec<HWND>,result:Option<Vec<String>>,done:bool}
unsafe extern "system" fn procedure(window:HWND,message:u32,w:WPARAM,l:LPARAM)->LRESULT{unsafe{
    if message==WM_NCCREATE{SetWindowLongPtrW(window,GWLP_USERDATA,(*(l as *const CREATESTRUCTW)).lpCreateParams as isize);}
    let pointer=GetWindowLongPtrW(window,GWLP_USERDATA) as *mut State;
    if pointer.is_null(){return DefWindowProcW(window,message,w,l);}
    let state=&mut *pointer;
    match message{
        WM_COMMAND if w&0xffff==1=>{
            state.result=Some(state.fields.iter().zip(&state.controls).map(|(field,control)|match field.kind{
                Kind::Check(_)=>(SendMessageW(*control,BM_GETCHECK,0,0)==BST_CHECKED as isize).then_some("1").unwrap_or("0").into(),
                Kind::Choice(..)=>SendMessageW(*control,CB_GETCURSEL,0,0).to_string(),
                Kind::Memo(_)=>String::new(),
                Kind::Text(_)=>{let mut buffer=vec![0u16;GetWindowTextLengthW(*control) as usize+1];GetWindowTextW(*control,buffer.as_mut_ptr(),buffer.len() as i32);String::from_utf16_lossy(&buffer[..buffer.len()-1])}
            }).collect());DestroyWindow(window);0
        },
        WM_COMMAND if w&0xffff==2=>{DestroyWindow(window);0},
        WM_COMMAND if matches!(w&0xffff,3|4)=>{
            for (field,control) in state.fields.iter().zip(&state.controls){if matches!(field.kind,Kind::Check(_)){SendMessageW(*control,BM_SETCHECK,if w&0xffff==3{BST_CHECKED}else{BST_UNCHECKED} as usize,0);}}0
        },
        WM_CLOSE=>{DestroyWindow(window);0},
        WM_DESTROY=>{state.done=true;0},
        WM_NCDESTROY=>{SetWindowLongPtrW(window,GWLP_USERDATA,0);DefWindowProcW(window,message,w,l)},
        _=>DefWindowProcW(window,message,w,l)
    }
}}
pub fn show(owner:HWND,title:&str,explanation:&str,fields:Vec<Field>)->Result<Option<Vec<String>>>{
    unsafe{
        let instance=GetModuleHandleW(null());let class=wide("CorpRustNativeForm");
        let wc=WNDCLASSW{lpfnWndProc:Some(procedure),hInstance:instance,hCursor:LoadCursorW(null_mut(),IDC_ARROW),hbrBackground:(COLOR_WINDOW+1) as HBRUSH,lpszClassName:class.as_ptr(),..zeroed()};
        RegisterClassW(&wc);
        let content_height=fields.iter().map(|field|if matches!(field.kind,Kind::Memo(_)){340}else{42}).sum::<i32>();
        let scale=(windows_sys::Win32::UI::HiDpi::GetDpiForWindow(owner).max(96) as f64/96.0)
            .min((GetSystemMetrics(SM_CYSCREEN)-60) as f64/(content_height as f64+172.0)).min((GetSystemMetrics(SM_CXSCREEN)-40) as f64/760.0).max(0.6);
        let px=|n:i32|(n as f64*scale) as i32;
        let font=CreateFontW(px(-16),0,0,0,400,0,0,0,DEFAULT_CHARSET as u32,0,0,CLEARTYPE_QUALITY as u32,0,wide("Microsoft YaHei UI").as_ptr());
        let mut state=Box::new(State{fields,controls:Vec::new(),result:None,done:false});
        let height=content_height+172;
        let window=CreateWindowExW(WS_EX_DLGMODALFRAME,class.as_ptr(),wide(title).as_ptr(),WS_OVERLAPPED|WS_CAPTION|WS_SYSMENU,
            CW_USEDEFAULT,CW_USEDEFAULT,px(760),px(height),owner,null_mut(),instance,(&mut *state as *mut State).cast());
        if window.is_null(){DeleteObject(font);return Err(win::error("创建选项窗口失败"));}
        let create=|id:i32,class:&str,text:&str,style:u32,x:i32,y:i32,width:i32,height:i32|{
            let control=CreateWindowExW(if class=="EDIT"{WS_EX_CLIENTEDGE}else{0},wide(class).as_ptr(),wide(text).as_ptr(),WS_CHILD|WS_VISIBLE|style,px(x),px(y),px(width),px(height),window,id as usize as HMENU,instance,null());
            SendMessageW(control,WM_SETFONT,font as usize,1);control
        };
        create(50,"STATIC",explanation,0,20,12,710,48);
        let mut y=72;
        for (index,field) in state.fields.iter().enumerate(){
            let id=100+index as i32;
            let control=match &field.kind{
                Kind::Check(value)=>{
                    let control=create(id,"BUTTON",&field.label,WS_TABSTOP|BS_AUTOCHECKBOX as u32,24,y,695,28);
                    SendMessageW(control,BM_SETCHECK,if *value{BST_CHECKED}else{BST_UNCHECKED} as usize,0);control
                },
                Kind::Text(value)=>{
                    create(200+id,"STATIC",&field.label,0,24,y+4,180,26);
                    let control=create(id,"EDIT",value,WS_TABSTOP|ES_AUTOHSCROLL as u32,208,y,510,30);
                    SendMessageW(control,EM_SETLIMITTEXT,32768,0);control
                },
                Kind::Memo(value)=>{
                    let control=create(id,"EDIT","",WS_TABSTOP|WS_VSCROLL|WS_HSCROLL|ES_MULTILINE as u32|ES_AUTOHSCROLL as u32|ES_AUTOVSCROLL as u32|ES_READONLY as u32,24,y,696,320);
                    SendMessageW(control,EM_SETLIMITTEXT,16*1024*1024,0);SetWindowTextW(control,wide(value).as_ptr());control
                },
                Kind::Choice(items,selected)=>{
                    create(200+id,"STATIC",&field.label,0,24,y+4,180,26);
                    let control=create(id,"COMBOBOX","",WS_TABSTOP|WS_VSCROLL|CBS_DROPDOWNLIST as u32,208,y,510,200);
                    for item in items{SendMessageW(control,CB_ADDSTRING,0,wide(item).as_ptr() as isize);}
                    SendMessageW(control,CB_SETCURSEL,*selected,0);control
                }
            };state.controls.push(control);
            y+=if matches!(field.kind,Kind::Memo(_)){340}else{42};
        }
        create(1,"BUTTON","确定",WS_TABSTOP|BS_DEFPUSHBUTTON as u32,486,height-88,110,34);
        create(2,"BUTTON","取消",WS_TABSTOP|BS_PUSHBUTTON as u32,610,height-88,110,34);
        if state.fields.len()>1&&state.fields.iter().all(|field|matches!(field.kind,Kind::Check(_))){
            create(3,"BUTTON","全选",WS_TABSTOP|BS_PUSHBUTTON as u32,24,height-88,76,34);
            create(4,"BUTTON","全不选",WS_TABSTOP|BS_PUSHBUTTON as u32,110,height-88,76,34);
        }
        EnableWindow(owner,0);ShowWindow(window,SW_SHOW);SetForegroundWindow(window);
        if let Some(control)=state.controls.first(){windows_sys::Win32::UI::Input::KeyboardAndMouse::SetFocus(*control);}
        let mut message:MSG=zeroed();let mut quit=None;
        while !state.done{
            let result=GetMessageW(&mut message,null_mut(),0,0);
            if result<=0{if result==0{quit=Some(message.wParam);}if IsWindow(window)!=0{DestroyWindow(window);}break;}
            if IsDialogMessageW(window,&message)==0{TranslateMessage(&message);DispatchMessageW(&message);}
        }
        EnableWindow(owner,1);SetForegroundWindow(owner);DeleteObject(font);
        if let Some(code)=quit{PostQuitMessage(code as i32);}
        Ok(state.result.take())
    }
}

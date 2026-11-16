use crate::{model::{Request,Result},win::{self,wide}};
use std::{mem::zeroed,path::{Path,PathBuf},ptr::{null,null_mut},time::{Duration,Instant}};
use windows_sys::Win32::{Foundation::*,Graphics::Gdi::*,System::LibraryLoader::GetModuleHandleW,UI::{HiDpi::GetDpiForSystem,Input::KeyboardAndMouse::EnableWindow,WindowsAndMessaging::*}};
struct State{next:bool,cancelled:bool,done:bool,allow_cancel:bool}
unsafe extern "system" fn procedure(window:HWND,message:u32,w:WPARAM,l:LPARAM)->LRESULT{unsafe{
    if message==WM_NCCREATE{SetWindowLongPtrW(window,GWLP_USERDATA,(*(l as *const CREATESTRUCTW)).lpCreateParams as isize);}
    let state=GetWindowLongPtrW(window,GWLP_USERDATA) as *mut State;if state.is_null(){return DefWindowProcW(window,message,w,l);}
    let state=&mut *state;
    match message{
        WM_COMMAND if w&0xffff==1=>{state.next=true;EnableWindow(GetDlgItem(window,1),0);SetWindowPos(window,HWND_NOTOPMOST,0,0,0,0,SWP_NOMOVE|SWP_NOSIZE|SWP_NOACTIVATE);0},
        WM_CLOSE|WM_COMMAND if message==WM_CLOSE||w&0xffff==2=>{
            if state.done{ShowWindow(window,SW_HIDE);}
            else if state.allow_cancel&&MessageBoxW(window,wide("确定要取消本次安装吗？").as_ptr(),wide("取消安装").as_ptr(),MB_YESNO|MB_DEFBUTTON2|MB_ICONQUESTION)==IDYES{state.cancelled=true;ShowWindow(window,SW_HIDE);}0
        },
        WM_CTLCOLORSTATIC=>{SetBkMode(w as HDC,TRANSPARENT as i32);GetStockObject(WHITE_BRUSH) as isize},
        WM_NCDESTROY=>{SetWindowLongPtrW(window,GWLP_USERDATA,0);DefWindowProcW(window,message,w,l)},
        _=>DefWindowProcW(window,message,w,l)
    }
}}
pub struct Wizard{window:HWND,state:Box<State>,cancel:PathBuf,fonts:Vec<HFONT>}
impl Wizard{
    pub fn open(request:&Request,package:&Path,directory:&Path)->Result<Self>{unsafe{
        let mut state=Box::new(State{next:false,cancelled:false,done:false,allow_cancel:request.install.allow_user_cancel});
        let instance=GetModuleHandleW(null());let class=wide("CorpRustInstallWizard");
        let wc=WNDCLASSW{lpfnWndProc:Some(procedure),hInstance:instance,hCursor:LoadCursorW(null_mut(),IDC_ARROW),hbrBackground:GetStockObject(WHITE_BRUSH) as HBRUSH,lpszClassName:class.as_ptr(),..zeroed()};RegisterClassW(&wc);
        let scale=GetDpiForSystem() as f64/96.0;let px=|value:i32|(value as f64*scale).round() as i32;
        let style=WS_OVERLAPPED|WS_CAPTION|WS_SYSMENU;let mut rect=RECT{left:0,top:0,right:px(604),bottom:px(348)};AdjustWindowRectEx(&mut rect,style,0,WS_EX_TOPMOST);
        let width=rect.right-rect.left;let height=rect.bottom-rect.top;
        let window=CreateWindowExW(WS_EX_TOPMOST,class.as_ptr(),wide("IT 软件安装").as_ptr(),style,(GetSystemMetrics(SM_CXSCREEN)-width)/2,(GetSystemMetrics(SM_CYSCREEN)-height)/2,width,height,null_mut(),null_mut(),instance,(&mut *state as *mut State).cast());
        if window.is_null(){return Err(win::error("创建 IT 软件安装向导失败"));}
        let fonts=[(14,400),(22,700),(27,700)].into_iter().map(|(size,weight)|CreateFontW(-px(size),0,0,0,weight,0,0,0,DEFAULT_CHARSET as u32,0,0,CLEARTYPE_QUALITY as u32,0,wide("Microsoft YaHei UI").as_ptr())).collect::<Vec<_>>();
        let product=if request.install.display_name.is_empty(){package.file_stem().unwrap_or_default().to_string_lossy().into_owned()}else{request.install.display_name.clone()};
        for (id,class,label,x,y,w,h,font) in [
            (10,"STATIC","IT 部门正在为你的电脑安装：".into(),36,34,540,34,1),(11,"STATIC",product,36,82,540,48,2),
            (12,"STATIC",format!("来源：IT 管理员（{}）",request.account),39,148,520,30,0),
            (13,"STATIC","安装程序将以指定管理员身份运行，并显示在当前桌面。请保存正在进行的工作。".into(),39,184,520,52,0),
            (14,"STATIC","准备就绪。点击“下一步”开始安装。".into(),39,235,520,52,0),
            (1,"BUTTON","下一步".into(),368,292,92,34,0),(2,"BUTTON","取消".into(),470,292,92,34,0)
        ]{
            let control=CreateWindowExW(0,wide(class).as_ptr(),wide(&label).as_ptr(),WS_CHILD|WS_VISIBLE|if class=="BUTTON"{WS_TABSTOP}else{0},px(x),px(y),px(w),px(h),window,id as usize as HMENU,instance,null());SendMessageW(control,WM_SETFONT,fonts[font] as usize,1);
        }
        EnableWindow(GetDlgItem(window,2),request.install.allow_user_cancel as i32);ShowWindow(window,SW_SHOW);SetForegroundWindow(window);
        Ok(Self{window,state,cancel:directory.join("cancel.signal"),fonts})
    }}
    pub fn pump(&mut self)->bool{unsafe{
        let mut message:MSG=zeroed();while PeekMessageW(&mut message,null_mut(),0,0,PM_REMOVE)!=0{
            if message.message==WM_QUIT{self.state.cancelled=true;PostQuitMessage(message.wParam as i32);break;}
            if IsDialogMessageW(self.window,&message)==0{TranslateMessage(&message);DispatchMessageW(&message);}
        }
        self.state.cancelled||self.cancel.exists()
    }}
    pub fn wait_for_confirmation(&mut self,timeout:Duration)->Result<bool>{
        let deadline=Instant::now()+timeout;
        while !self.state.next{if self.pump(){return Ok(false);}if Instant::now()>deadline{return Err("等待目标用户确认安装超时，未启动安装器。".into());}std::thread::sleep(Duration::from_millis(40));}
        self.status("正在启动安装程序，请按安装向导完成操作…");Ok(true)
    }
    pub fn status(&self,text:&str){unsafe{
        let label = GetDlgItem(self.window,14);
        SetWindowTextW(label,wide(text).as_ptr());
        UpdateWindow(label);
    }}
    pub fn finish(&mut self,text:&str){
        self.state.done=true;self.status(text);unsafe{SetWindowTextW(GetDlgItem(self.window,2),wide("关闭").as_ptr());EnableWindow(GetDlgItem(self.window,2),1);}
        let end=Instant::now()+Duration::from_millis(900);while Instant::now()<end{self.pump();std::thread::sleep(Duration::from_millis(30));}
    }
}
impl Drop for Wizard{fn drop(&mut self){unsafe{DestroyWindow(self.window);for font in &self.fonts{DeleteObject(*font);}}}}

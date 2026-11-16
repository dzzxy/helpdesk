mod pages;
mod tables;
mod canvas;
mod guidance;
mod session_selection;
mod suggestions;
mod splitter;
use pages::*;
use crate::{model::{self, Credentials, Request, Result}, remote::{self, Reporter}, win::{self, wide}};
use std::{cell::Cell, fs, mem::{size_of, zeroed}, ptr::{null, null_mut}, sync::{Arc, atomic::{AtomicBool, Ordering}}, time::Instant};
use windows_sys::Win32::{Foundation::*, Graphics::Gdi::*, System::{SystemServices::SS_LEFT, LibraryLoader::*}, UI::{Input::KeyboardAndMouse::{EnableWindow,IsWindowEnabled}, WindowsAndMessaging::*, Controls::{*, Dialogs::*}, HiDpi::*}};
use zeroize::Zeroize;

const WM_LOG:u32=WM_APP+1;
const WM_DONE:u32=WM_APP+2;
const NAV:i32=301;
const NAV_MAINTENANCE:i32=302;
const CLEAR_TARGETS:i32=751;
const WINDOW_TITLE:&str=if cfg!(debug_assertions){"企业软件部署 · Rust 0.1.15 调试核对（开发中）"}else{"企业软件部署 · Rust 0.1.15 复刻核对版（开发中）"};
const RUN:i32=201;
const CANCEL:i32=202;
const SESSIONS:i32=203;
const BROWSE:i32=204;
const EXPORT:i32=205;
const SECONDARY:i32=206;
const THIRD:i32=207;
const TARGET:i32=101;
const ACCOUNT:i32=102;
const PASSWORD:i32=103;
const SOURCE:i32=104;
const DISTRIBUTION_SOURCE:i32=113;
const EXECUTION_MODE:i32=114;
const INVENTORY_CONCURRENCY:i32=115;
const ARGUMENTS:i32=105;
const SESSION:i32=106;
const TIMEOUT:i32=107;
const DESTINATION:i32=108;
const KEY:i32=109;
const VALUE:i32=110;
const EXISTING:i32=111;
const OVERWRITE:i32=112;
const LOG:i32=401;
const TITLE:i32=402;
const HINT:i32=403;
const STATUS:i32=404;
const SAVE_ACCOUNT:i32=208;
const DELETE_ACCOUNT:i32=209;
const WM_SESSIONS:u32=WM_APP+3;
const WM_INVENTORY:u32=WM_APP+4;
const TASK_HISTORY:i32=210;
const TASK_CENTER:i32=211;
const NAVIGATION:[(i32,&str);10]=[(0,"安装任务"),(1,"文件夹分发"),(3,"电脑信息收集"),(7,"软件卸载"),(-1,"维护"),(4,"提权打开无窗口显示"),(8,"安全日志维护"),(9,"日志诊断收集"),(5,"连接诊断与修复"),(10,"软件提权")];
const REGISTRY_VIEW:i32=116;
const INVENTORY_OPTIONS:i32=600;
const RESULT_TABLE:i32=405;
const LOG_RANGE:i32=117;
const LOG_OPTIONS:i32=620;
const PAGE_COUNT:usize=12;
const WM_RESULT:u32=WM_APP+6;
const WM_PROGRESS:u32=WM_APP+7;
const SOFTWARE_SEARCH:i32=118;
const ELEVATION_PATH:i32=119;
const ELEVATION_TRUST:i32=120;
const ELEVATION_UPDATE:i32=121;
const REMEMBER_CREDENTIALS:i32=122;

fn history_directory() -> Result<std::path::PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA").ok_or("找不到本地应用数据目录。")?;
    Ok(std::path::PathBuf::from(base).join("CorpInstallerRust").join("History"))
}

fn save_summary(summary: &str) -> Result<()> {
    let directory = history_directory()?;
    win::io(fs::create_dir_all(&directory))?;
    use std::io::Write;
    let mut file = win::io(fs::OpenOptions::new().create_new(true).write(true)
        .open(directory.join(format!("{}.txt", model::new_id()))))?;
    win::io(file.write_all(summary.as_bytes()))
}

fn select_local_folder(owner: HWND) -> Option<String> {
    use windows_sys::Win32::UI::Shell::*;
    let title = wide("选择要分发的本地文件夹");
    let browse = BROWSEINFOW {
        hwndOwner: owner,
        lpszTitle: title.as_ptr(),
        ulFlags: BIF_RETURNONLYFSDIRS,
        ..Default::default()
    };
    let mut path = [0u16; 260];
    unsafe {
        let item = SHBrowseForFolderW(&browse);
        if item.is_null() { return None; }
        let success = SHGetPathFromIDListW(item, path.as_mut_ptr());
        // PIDLs returned by the shell use the COM task allocator.
        #[link(name = "ole32")]
        unsafe extern "system" { fn CoTaskMemFree(pointer: *mut std::ffi::c_void); }
        CoTaskMemFree(item.cast());
        (success != 0).then(|| win::string(path.as_ptr()))
    }
}

struct Control { id:i32, handle:HWND, x:i32,y:i32,w:i32,h:i32,page:i32 }
struct Batch {id:String,cancel:Arc<AtomicBool>,started:Instant}
struct Ui { suggestions:suggestions::Suggestions, guidance:guidance::Guidance, window:HWND, canvas:HWND, layout_in_progress:Cell<bool>, controls:Vec<Control>, font:HFONT, title_font:HFONT, page:i32, busy:bool, closing:bool, cancel:Arc<AtomicBool>, logs:[String;PAGE_COUNT], statuses:[String;PAGE_COUNT], batches:std::collections::HashMap<usize,Batch>, scale:f64, started:Instant, sessions:Vec<(String,String,u32)>, inventory: Vec<serde_json::Value>, records:[Vec<serde_json::Value>;PAGE_COUNT], history:Vec<crate::tasks::Entry>, junk_host:String, junk_items:Vec<serde_json::Value>, junk_selection:Vec<serde_json::Value>, review_pending:bool, elevation_preview:Option<serde_json::Value>, approve_pending:bool, retry_request:Option<(Vec<String>,Request)>, page_state:pages::State, _not_send:Cell<()> }
impl Ui {
    unsafe fn add(&mut self,id:i32,class:&str,text:&str,style:u32,x:i32,y:i32,w:i32,h:i32,page:i32) {
        unsafe {
            let style=if class=="BUTTON" && (style & BS_TYPEMASK as u32)<=BS_DEFPUSHBUTTON as u32 {
                (style & !(BS_TYPEMASK as u32))|BS_OWNERDRAW as u32
            }else if class=="COMBOBOX"{style|CBS_OWNERDRAWFIXED as u32|CBS_HASSTRINGS as u32}else{style};
            let handle=CreateWindowExW(if class=="EDIT" {WS_EX_CLIENTEDGE}else{0},wide(class).as_ptr(),wide(text).as_ptr(),WS_CHILD|WS_VISIBLE|WS_CLIPSIBLINGS|style,
                0,0,0,0,self.window,id as usize as HMENU,GetModuleHandleW(null()),null());
            SendMessageW(handle,WM_SETFONT,if id==TITLE {self.title_font}else{self.font} as usize,1);
            if class=="COMBOBOX"{
                SendMessageW(handle,CB_SETITEMHEIGHT,usize::MAX,(30.0*self.scale).round() as isize);
                SendMessageW(handle,CB_SETITEMHEIGHT,0,(26.0*self.scale).round() as isize);
                if matches!(id,SOURCE|DISTRIBUTION_SOURCE){SendMessageW(handle,CB_SETDROPPEDWIDTH,(850.0*self.scale).min((GetSystemMetrics(SM_CXSCREEN)-40) as f64) as usize,0);}
            }
            if class=="EDIT" {SendMessageW(handle,EM_SETLIMITTEXT,if id==LOG {1024*1024}else{32768},0);}
            self.controls.push(Control{id,handle,x,y,w,h,page});
        }
    }
    fn handle(&self,id:i32)->HWND {self.controls.iter().find(|c|c.id==id).map_or(null_mut(),|c|c.handle)}
    fn text(&self,id:i32)->String {unsafe{let h=self.handle(id);let mut b=vec![0;GetWindowTextLengthW(h) as usize+1];GetWindowTextW(h,b.as_mut_ptr(),b.len() as i32);String::from_utf16_lossy(&b[..b.len()-1])}}
    fn set(&self,id:i32,text:&str) {unsafe{SetWindowTextW(self.handle(id),wide(text).as_ptr());}}
    fn checked(&self,id:i32)->bool {unsafe{SendMessageW(self.handle(id),BM_GETCHECK,0,0)==BST_CHECKED as isize}}
    fn build(&mut self) {
        unsafe {
            self.add(501,"STATIC","操作台",SS_LEFT,16,24,170,28,-1);
            self.add(502,"STATIC","Rust 核对版 0.1.15",SS_LEFT,24,59,180,24,-1);
            self.add(526,"STATIC","部署",SS_LEFT,16,72,170,22,-1);
            self.add(TASK_CENTER,"BUTTON","任务中心",WS_TABSTOP,10,590,184,42,-1);
            self.add(527,"STATIC","● 管理端就绪\r\n免常驻 Agent\r\n临时服务 · SYSTEM 权限",SS_LEFT,12,650,182,72,-1);
            self.add(NAV,"LISTBOX","",LBS_NOTIFY as u32|LBS_OWNERDRAWFIXED as u32|LBS_HASSTRINGS as u32|LBS_NOINTEGRALHEIGHT as u32|WS_TABSTOP,10,98,184,168,-1);
            self.add(530,"STATIC","维护",SS_LEFT,16,284,170,22,-1);
            self.add(NAV_MAINTENANCE,"LISTBOX","",LBS_NOTIFY as u32|LBS_OWNERDRAWFIXED as u32|LBS_HASSTRINGS as u32|LBS_NOINTEGRALHEIGHT as u32|WS_TABSTOP,10,310,184,210,-1);
            for (id,items) in [(NAV,&NAVIGATION[..4]),(NAV_MAINTENANCE,&NAVIGATION[5..])]{
                for (_,name) in items{SendMessageW(self.handle(id),LB_ADDSTRING,0,wide(name).as_ptr() as isize);}
                SendMessageW(self.handle(id),LB_SETITEMHEIGHT,0,(42.0*self.scale) as isize);
            }
            SendMessageW(self.handle(NAV),LB_SETCURSEL,0,0);
            windows_sys::Win32::UI::Shell::SetWindowSubclass(self.handle(NAV),Some(navigation_keys),1,self.handle(NAV_MAINTENANCE) as usize);
            windows_sys::Win32::UI::Shell::SetWindowSubclass(self.handle(NAV_MAINTENANCE),Some(navigation_keys),1,self.handle(NAV) as usize);
            self.add(TITLE,"STATIC","软件安装 / 直接运行",SS_LEFT,236,25,900,36,-1);
            self.add(505,"STATIC","目标电脑（多个目标用逗号或空格分隔）",SS_LEFT,236,105,700,22,-1);
            self.add(TARGET,"EDIT","",WS_TABSTOP|WS_VSCROLL|ES_MULTILINE as u32|ES_AUTOVSCROLL as u32|ES_WANTRETURN as u32,236,131,890,29,-1);
            self.add(506,"STATIC","管理员账号",SS_LEFT,236,174,160,22,-1);
            self.add(ACCOUNT,"COMBOBOX","",WS_TABSTOP|WS_VSCROLL|CBS_DROPDOWN as u32|CBS_AUTOHSCROLL as u32,360,169,290,225,-1);
            self.add(507,"STATIC","密码",SS_LEFT,665,174,45,22,-1);
            self.add(PASSWORD,"EDIT","",WS_TABSTOP|ES_PASSWORD as u32|ES_AUTOHSCROLL as u32,712,169,226,29,-1);
            self.add(SAVE_ACCOUNT,"BUTTON","保存",WS_TABSTOP,948,169,82,29,-1);
            self.add(DELETE_ACCOUNT,"BUTTON","删除",WS_TABSTOP,1044,169,82,29,-1);
            self.add(HINT,"STATIC","",SS_LEFT,236,215,895,43,-1);
            self.add(510,"STATIC","安装文件",SS_LEFT,236,273,105,24,0);
            self.add(SOURCE,"COMBOBOX","",WS_TABSTOP|WS_VSCROLL|CBS_DROPDOWN as u32|CBS_AUTOHSCROLL as u32,350,268,650,240,0);
            self.add(DISTRIBUTION_SOURCE,"COMBOBOX","",WS_TABSTOP|WS_VSCROLL|CBS_DROPDOWN as u32|CBS_AUTOHSCROLL as u32,350,268,650,240,1);
            self.add(BROWSE,"BUTTON","浏览文件…",WS_TABSTOP|BS_PUSHBUTTON as u32,1010,268,116,30,-2);
            self.add(511,"STATIC","安装参数",SS_LEFT,236,317,105,24,0);
            self.add(ARGUMENTS,"EDIT","",WS_TABSTOP|ES_AUTOHSCROLL as u32,350,312,776,30,0);
            self.add(512,"STATIC","登录会话",SS_LEFT,236,361,105,24,-3);
            self.add(SESSION,"COMBOBOX","",WS_TABSTOP|WS_VSCROLL|CBS_DROPDOWNLIST as u32,350,356,400,225,-3);
            self.add(SESSIONS,"BUTTON","检测会话",WS_TABSTOP|BS_PUSHBUTTON as u32,760,356,135,30,-3);
            self.add(513,"STATIC","超时 / 秒",SS_LEFT,912,361,92,24,-3);
            self.add(TIMEOUT,"EDIT","60",WS_TABSTOP|ES_NUMBER as u32,1010,356,115,30,-3);
            self.add(EXISTING,"BUTTON","使用目标机已有文件",WS_TABSTOP|BS_AUTOCHECKBOX as u32,635,400,400,30,0);
            self.add(520,"STATIC","运行方式",SS_LEFT,236,405,105,24,0);
            self.add(EXECUTION_MODE,"COMBOBOX","",WS_TABSTOP|WS_VSCROLL|CBS_DROPDOWNLIST as u32,350,400,260,160,0);
            for mode in ["安装软件", "直接运行（免安装 EXE）"] {
                SendMessageW(self.handle(EXECUTION_MODE), CB_ADDSTRING, 0, wide(mode).as_ptr() as isize);
            }
            SendMessageW(self.handle(EXECUTION_MODE), CB_SETCURSEL, 0, 0);
            self.add(514,"STATIC","本地文件 / 目录",SS_LEFT,236,273,120,24,1);
            self.add(515,"STATIC","目标目录",SS_LEFT,236,317,110,24,1);
            self.add(DESTINATION,"EDIT","",WS_TABSTOP|ES_AUTOHSCROLL as u32,350,312,776,30,1);
            self.add(522,"STATIC","冲突处理",SS_LEFT,236,361,110,24,1);
            self.add(OVERWRITE,"COMBOBOX","",WS_TABSTOP|WS_VSCROLL|CBS_DROPDOWNLIST as u32,350,356,360,175,1);
            for policy in ["逐个询问", "覆盖全部", "跳过全部", "仅覆盖源文件较新", "仅覆盖 SHA-256 不同"] {
                SendMessageW(self.handle(OVERWRITE), CB_ADDSTRING, 0, wide(policy).as_ptr() as isize);
            }
            SendMessageW(self.handle(OVERWRITE), CB_SETCURSEL, 4, 0);
            self.add(516,"STATIC","注册表路径",SS_LEFT,236,273,110,24,4);
            self.add(KEY,"EDIT","HKLM\\SOFTWARE",WS_TABSTOP|ES_AUTOHSCROLL as u32,350,268,600,30,4);
            self.add(REGISTRY_VIEW,"COMBOBOX","",WS_TABSTOP|WS_VSCROLL|CBS_DROPDOWNLIST as u32,966,268,160,120,4);
            for view in ["64 位视图", "32 位视图"] {
                SendMessageW(self.handle(REGISTRY_VIEW), CB_ADDSTRING, 0, wide(view).as_ptr() as isize);
            }
            SendMessageW(self.handle(REGISTRY_VIEW), CB_SETCURSEL, 0, 0);
            self.add(517,"STATIC","DWORD 数据",SS_LEFT,236,327,110,24,4);
            self.add(VALUE,"EDIT","1",WS_TABSTOP|ES_AUTOHSCROLL as u32,350,322,190,30,4);
            self.add(518,"STATIC","TransparentEnabled：写入上方账号的 HKCU，需账号配置已加载。",SS_LEFT,350,368,760,45,4);
            self.add(528,"STATIC","输入非负整数，例如 1；十六进制可写成 0x1。\r\n\r\n本功能固定写入目标账号的注册表项：\r\nHKEY_CURRENT_USER\\Software\\Policies\\\r\nMicrosoft\\Windows\\Safer\\CodeIdentifiers\r\n值名 TransparentEnabled（REG_DWORD）。",SS_LEFT,0,0,1,1,4);
            self.add(529,"STATIC","企业运维 / 工作台",SS_LEFT,236,10,600,20,4);
            self.add(REMEMBER_CREDENTIALS,"BUTTON","执行修改时保存到 Windows 凭据管理器",WS_TABSTOP|BS_AUTOCHECKBOX as u32,236,382,500,26,4);
            self.add(521,"STATIC","并发电脑数",SS_LEFT,236,273,110,24,3);
            self.add(INVENTORY_CONCURRENCY,"COMBOBOX","",WS_TABSTOP|WS_VSCROLL|CBS_DROPDOWNLIST as u32,350,268,125,160,3);
            for count in ["1", "2", "4", "8", "12", "16"] {
                SendMessageW(self.handle(INVENTORY_CONCURRENCY), CB_ADDSTRING, 0, wide(count).as_ptr() as isize);
            }
            SendMessageW(self.handle(INVENTORY_CONCURRENCY), CB_SETCURSEL, 2, 0);
            self.add(523,"STATIC","收集范围",SS_LEFT,236,273,110,24,9);
            self.add(LOG_RANGE,"COMBOBOX","",WS_TABSTOP|WS_VSCROLL|CBS_DROPDOWNLIST as u32,350,268,220,160,9);
            for days in ["最近 24 小时","最近 3 天","最近 7 天","最近 30 天"] {SendMessageW(self.handle(LOG_RANGE),CB_ADDSTRING,0,wide(days).as_ptr() as isize);}
            SendMessageW(self.handle(LOG_RANGE),CB_SETCURSEL,2,0);
            self.add(524,"STATIC","筛选软件",SS_LEFT,236,273,110,24,7);
            self.add(SOFTWARE_SEARCH,"EDIT","",WS_TABSTOP|ES_AUTOHSCROLL as u32,350,268,776,30,7);
            self.add(525,"STATIC","目标软件",SS_LEFT,236,273,110,24,10);
            self.add(ELEVATION_PATH,"EDIT","",WS_TABSTOP|ES_AUTOHSCROLL as u32,350,268,776,30,10);
            self.add(ELEVATION_TRUST,"BUTTON","确认此软件、配置和插件可信",WS_TABSTOP|BS_AUTOCHECKBOX as u32,350,310,430,28,10);
            self.add(ELEVATION_UPDATE,"BUTTON","更新或替换目标机已有运行账号及密码",WS_TABSTOP|BS_AUTOCHECKBOX as u32,790,310,320,28,10);
            for (index,name) in ["卡顿/性能","蓝屏/重启","软件崩溃","安装失败","Windows/诊断事件","可靠性历史","原始 EVTX","蓝屏小型转储"].iter().enumerate() {
                let id=LOG_OPTIONS+index as i32;
                self.add(id,"BUTTON",name,WS_TABSTOP|BS_AUTOCHECKBOX as u32,236+(index%4) as i32*210,315+(index/4) as i32*38,190,28,9);
                SendMessageW(self.handle(id),BM_SETCHECK,BST_CHECKED as usize,0);
            }
            for (index,name) in ["系统","硬件","磁盘","网络","会话","软件","补丁","服务"].iter().enumerate() {
                let id=INVENTORY_OPTIONS+index as i32;
                self.add(id,"BUTTON",name,WS_TABSTOP|BS_AUTOCHECKBOX as u32,236+(index%4) as i32*210,315+(index/4) as i32*38,180,28,3);
                SendMessageW(self.handle(id),BM_SETCHECK,BST_CHECKED as usize,0);
            }
            self.add(RUN,"BUTTON","开始安装",WS_TABSTOP|BS_DEFPUSHBUTTON as u32,236,452,160,37,-1);
            self.add(SECONDARY,"BUTTON","直接运行 EXE",WS_TABSTOP|BS_PUSHBUTTON as u32,410,452,170,37,-1);
            self.add(THIRD,"BUTTON","写入固定值",WS_TABSTOP|BS_PUSHBUTTON as u32,594,452,170,37,-1);
            self.add(CANCEL,"BUTTON","取消任务",WS_TABSTOP|BS_PUSHBUTTON as u32,790,452,150,37,-1);
            self.add(EXPORT,"BUTTON","导出结果…",WS_TABSTOP|BS_PUSHBUTTON as u32,954,452,172,37,-1);
            self.add(STATUS,"STATIC","任务结果 · 就绪",SS_LEFT,236,509,890,24,-1);
            self.add(RESULT_TABLE,"SysListView32","",WS_TABSTOP|LVS_REPORT|LVS_SINGLESEL|LVS_SHOWSELALWAYS,236,543,890,160,-1);
            SendMessageW(self.handle(RESULT_TABLE),LVM_SETEXTENDEDLISTVIEWSTYLE,0,(LVS_EX_FULLROWSELECT|LVS_EX_DOUBLEBUFFER) as isize);
            windows_sys::Win32::UI::Shell::SetWindowSubclass(self.handle(RESULT_TABLE),Some(win::listview_scroll),2,0);
            for (index,(title,width)) in [("目标电脑",155),("状态",130),("操作",145),("信息",440)].iter().enumerate() {
                let mut title=wide(title);
                let column=LVCOLUMNW{mask:LVCF_TEXT|LVCF_WIDTH,cx:(*width as f64*self.scale) as i32,pszText:title.as_mut_ptr(),..zeroed()};
                SendMessageW(self.handle(RESULT_TABLE),LVM_INSERTCOLUMNW,index,(&column as *const LVCOLUMNW) as isize);
            }
            self.add(LOG,"EDIT","",WS_TABSTOP|WS_VSCROLL|WS_HSCROLL|ES_MULTILINE as u32|ES_READONLY as u32|ES_AUTOVSCROLL as u32|ES_AUTOHSCROLL as u32,236,543,890,235,-1);
            self.add(519,"STATIC","关闭窗口会取消当前任务，临时文件和服务将在后台清理。",SS_LEFT,236,798,890,23,-1);
            EnableWindow(self.handle(CANCEL),0);
            self.build_pages();
            splitter::register();
            self.add(splitter::CONTROL, splitter::CLASS, "上下拖动调整结果区域", WS_TABSTOP, 236, 0, 1, 1, 3);
            ShowWindow(self.handle(splitter::CONTROL), SW_HIDE);
            for id in [828,843]{SendMessageW(self.handle(id),WM_SETFONT,self.title_font as usize,1);}
            self.add(CLEAR_TARGETS,"BUTTON","清空目标",WS_TABSTOP|BS_PUSHBUTTON as u32,1004,34,122,32,-1);
            canvas::attach(self);
            guidance::Guidance::attach(self);
        }
        let preferred=crate::credentials::preferred().unwrap_or_default();
        self.reload_accounts(&preferred);
        if let Ok(password)=crate::credentials::read(&preferred) {
            let password=zeroize::Zeroizing::new(password);
            self.set(PASSWORD,&password);
        }
        self.select_page(0);self.layout();
        self.append("Rust 源码核对版：任务结果、错误原因和目标机回执显示在对应页面。\r\n远程安装、卸载、授权和日志维护的实际效果仍需按验证记录逐项核实。".into());
    }
    fn layout(&self) {
        guidance::Guidance::cues(self);
        if self.layout_in_progress.replace(true){return;}
        struct LayoutGuard<'a>(&'a Ui);
        impl Drop for LayoutGuard<'_>{fn drop(&mut self){unsafe{
            SendMessageW(self.0.canvas,WM_SETREDRAW,1,0);
            self.0.layout_in_progress.set(false);
            RedrawWindow(self.0.window,null(),null_mut(),RDW_INVALIDATE|RDW_ERASE|RDW_ALLCHILDREN|RDW_FRAME|RDW_UPDATENOW);
        }}}
        let _layout=LayoutGuard(self);
        unsafe {
            SendMessageW(self.canvas,WM_SETREDRAW,0,0);
            let mut rect=zeroed();GetClientRect(self.window,&mut rect);
            let physical_bottom=rect.bottom;canvas::layout(self,&mut rect);
            let sx=|n:i32|(n as f64*self.scale).round() as i32;
            let extra_w=rect.right-sx(1150);
            let action_offset=match self.page { 1 => -40, 4 => -20, 2|8|5 => -175, 7|10 => -35, 3|9 => -45, 6|11 => -308, _ => 0 };
            for c in &self.controls {
                if !matches!(self.page,2|6)&&GetParent(c.handle)==self.canvas{continue;}
                let stretch=if [TARGET,LOG,TITLE,HINT,STATUS,519,SOURCE,DISTRIBUTION_SOURCE,ARGUMENTS,DESTINATION,KEY,PASSWORD].contains(&c.id){extra_w}else{0};
                let right_align=if [CANCEL,EXPORT,BROWSE,SAVE_ACCOUNT,DELETE_ACCOUNT,REGISTRY_VIEW].contains(&c.id){extra_w}else{0};
                let mut y=sx(c.y);
                if c.y>=452 && c.x>=236 {y+=sx(action_offset);}
                if c.id==HINT && matches!(self.page,6|11) {y=sx(68);}
                if [TARGET,505].contains(&c.id){y-=sx(45);}
                if [ACCOUNT,PASSWORD,506,507,SAVE_ACCOUNT,DELETE_ACCOUNT].contains(&c.id){y-=sx(34);}
                if c.id==HINT && !matches!(self.page,6|11) {y-=sx(28);}
                if c.y>=268 && c.y<452 && c.x>=236 {y-=sx(28);}
                if c.y>=452 && c.x>=236 {y-=sx(24);}
                if c.id==519 {y=rect.bottom-sx(31);}
                if c.id==TASK_CENTER{y=(physical_bottom-sx(170)).max(sx(548));}
                if c.id==527{y=(physical_bottom-sx(104)).max(sx(610));}
                if c.id==LOG {y+=sx(170);}
                let h=if c.id==LOG {(rect.bottom-y-sx(45)).max(sx(60))}else{sx(c.h)};
                canvas::move_control(self,c.handle,sx(c.x)+right_align,y,(sx(c.w)+stretch).max(sx(80)),h);
                let visible=(c.page<0 && (c.page!=-2 || self.page<=1) && (c.page!=-3 || matches!(self.page,0|7|10)) || c.page==self.page)
                    && !(matches!(self.page,6|11) && [ACCOUNT,PASSWORD,506,507,SAVE_ACCOUNT,DELETE_ACCOUNT].contains(&c.id))
                    && !(matches!(self.page,6|11) && [TARGET,505].contains(&c.id));
                ShowWindow(c.handle,if visible&&c.id!=502{SW_SHOW}else{SW_HIDE});
            }
            ShowWindow(self.handle(SECONDARY),if matches!(self.page,1|3|4|5|7|8|9|10|11){SW_SHOW}else{SW_HIDE});
            ShowWindow(self.handle(THIRD),if matches!(self.page,4|7|10){SW_SHOW}else{SW_HIDE});
            EnableWindow(self.handle(RUN),(!self.busy&&self.page!=6) as i32);
            for id in [SECONDARY,THIRD,SESSIONS]{EnableWindow(self.handle(id),(!self.busy) as i32);}
            EnableWindow(self.handle(CANCEL),(self.busy||self.page==11) as i32);
            if self.page==4{self.layout_registry(&rect);}
            if self.page==1{self.layout_distribution(&rect);}
            if self.page==3{self.layout_inventory(&rect);}
            if self.page==5{self.layout_diagnostics(&rect);}
            if self.page==7{self.layout_bcu(&rect);}
            if self.page==8{self.layout_security(&rect);}
            if self.page==9{self.layout_logs(&rect);}
            if self.page==0{self.layout_install(&rect);}
            if self.page==11{self.layout_tasks(&rect);}
            if self.page==10{self.layout_elevation(&rect);}
            let mut actual:RECT=zeroed();GetClientRect(self.window,&mut actual);
            canvas::move_control(self,self.handle(TITLE),sx(225),sx(34),(actual.right-sx(383)).max(sx(180)),sx(36));
            canvas::move_control(self,self.handle(CLEAR_TARGETS),actual.right-sx(146),sx(34),sx(122),sx(32));
            ShowWindow(self.handle(CLEAR_TARGETS),SW_SHOW);
            InvalidateRect(self.canvas,null(),1);
        }
    }
    unsafe fn layout_registry(&self,rect:&RECT){unsafe{
        let px=|n:i32|(n as f64*self.scale).round() as i32;
        let left=225;let width=((rect.right as f64/self.scale) as i32-left-20).max(600);
        let half=(width-22)/2;let right=left+half+22;
        for control in self.controls.iter().filter(|control|GetParent(control.handle)==self.canvas||matches!(control.id,TITLE|529)){
            ShowWindow(control.handle,SW_HIDE);
        }
        let place=|id,x,y,w,h|{canvas::move_control(self,self.handle(id),px(x),px(y),px(w),px(h));ShowWindow(self.handle(id),SW_SHOW);};
        place(529,left,10,width,20);place(TITLE,left,34,width,36);
        place(505,left,106,half,24);place(TARGET,left,132,half,100);
        place(506,left,244,half,24);place(ACCOUNT,left,270,half,225);
        place(507,left,310,half,24);place(PASSWORD,left,336,half,38);
        place(REMEMBER_CREDENTIALS,left,382,half,28);
        place(517,right,106,half,24);place(VALUE,right,132,half,38);place(528,right,180,half,156);
        place(HINT,left,422,width,24);place(RUN,left,454,112,38);place(SECONDARY,left+124,454,112,38);
        place(STATUS,left+250,460,width-250,28);
        place(RESULT_TABLE,left,508,width,((rect.bottom as f64/self.scale) as i32-530).max(120));
        for id in [RUN,SECONDARY]{EnableWindow(self.handle(id),(!self.busy) as i32);}
    }}
    fn select_page(&mut self,page:i32) {
        if !(0..PAGE_COUNT as i32).contains(&page) { return; }
        if self.page!=page{self.invalidate_suggestions();}
        self.statuses[self.page as usize] = self.text(STATUS);
        self.page_state.remember[self.page as usize]=self.checked(REMEMBER_CREDENTIALS);
        self.page_state.timeouts[self.page as usize]=self.text(TIMEOUT);
        self.page=page;
        self.set(TIMEOUT,&self.page_state.timeouts[page as usize]);
        self.set(513,if page==5{"检测超时（秒）"}else{"单机超时（分钟）"});
        unsafe{SendMessageW(self.handle(REMEMBER_CREDENTIALS),BM_SETCHECK,if self.page_state.remember[page as usize]{BST_CHECKED}else{BST_UNCHECKED} as usize,0);}
        unsafe{
            SendMessageW(self.handle(NAV),LB_SETCURSEL,NAVIGATION[..4].iter().position(|(id,_)|*id==page).unwrap_or(usize::MAX),0);
            SendMessageW(self.handle(NAV_MAINTENANCE),LB_SETCURSEL,NAVIGATION[5..].iter().position(|(id,_)|*id==page).unwrap_or(usize::MAX),0);
        }
        self.busy=self.batches.contains_key(&(page as usize));
        if let Some(batch)=self.batches.get(&(page as usize)){self.cancel=batch.cancel.clone();self.started=batch.started;}
        self.set(STATUS, &self.statuses[page as usize]);
        self.refresh_log();
        let (title,hint,button)=match page {
            0=>("安装任务","检测后可选择目标电脑的登录会话；存在多个会话时，会提示逐台选择。", "开始安装"),
            1=>("文件夹分发","目录会分发其中的内容；文件放到目标目录下。默认仅覆盖 SHA-256 不同的文件。","开始分发"),
            2=>("登录会话","通过原生 WTS 查询当前登录账号、会话 ID 和状态；查询不会上传组件。","检测登录会话"),
            3=>("电脑信息收集","通过一次性 Rust 服务读取 OS、CPU、内存、磁盘和登录会话。需要 ADMIN$ 与 SCM 权限。","收集电脑信息"),
            4=>("提权打开无窗口显示","选择 64 位或 32 位视图浏览 HKLM/HKU。固定值检测和写入使用原有视图。","浏览注册表"),
            5=>("连接诊断与修复","无需凭据可测 DNS、Ping 和端口；提供凭据时加测 ADMIN$/SCM、WMI、WinRM 权限。修复使用管理员凭据，首次 SSH 连接需核对指纹。","开始诊断"),
            7=>("软件卸载","扫描目标电脑上的软件后选择卸载项目。","扫描软件"),
            8=>("安全日志维护","读取日志记录数量和覆盖配置，清理前需确认。","读取日志状态"),
            9=>("日志诊断收集","选择范围和类别，诊断包保存到管理机。","收集诊断日志"),
            10=>("软件提权","管理目标电脑的软件授权启动。","查看授权"),
            11=>("任务中心","查看任务状态、目标电脑和执行结果。","刷新任务"),
            _=>("功能覆盖说明","已实现功能见下方；此版本用于 Rust 原生性能与常用流程测试，尚不是 v80 完整替代版。","查看说明"),
        };
        self.set(TITLE,title);self.set(HINT,hint);self.set(RUN,button);
        self.set(SAVE_ACCOUNT,if page==10{"保存账号密码"}else{"保存"});self.set(DELETE_ACCOUNT,if page==10{"删除凭据配置"}else{"删除"});
        self.set(BROWSE, if page == 0 && self.checked(EXISTING) { "浏览目标机…" } else { "浏览文件…" });
        self.set(SECONDARY,match page{1=>"选择文件夹…",3=>"导出电脑信息…",5=>"修复连接",7=>"卸载选中软件",8=>"清理并自动覆盖",9=>"打开诊断包目录",10=>"预览并创建授权",11=>"重试选中任务",_=>"检测固定值"});
        self.set(THIRD,if page==10{"撤销选中授权"}else if page==7{"审核卸载残留…"}else{"写入固定值"});
        self.set(505,match page{4=>"目标 IP 或计算机名（每行一台）",5=>"目标电脑 / IP（每行一台）",7=>"目标电脑",8=>"目标计算机名 / IP",10=>"IP 或主机名",_=>"目标电脑（每行一台）"});
        self.set(506,if page==4{"目标账号（写入其 HKCU）"}else{"管理员账号"});
        self.set(507,if page==4{"管理员密码"}else{"密码"});
        self.set(517,if page==4{"要写入的值"}else{"DWORD 数据"});
        self.set(REMEMBER_CREDENTIALS,if page==4{"执行修改时保存到 Windows 凭据管理器"}else{"保存目标凭据到 Credential Manager"});
        if page==1{self.set(505,"目标电脑（每行一台）");self.set(506,"目标管理员账号");self.set(507,"目标管理员密码");self.set(BROWSE,"选择文件…");self.set(SECONDARY,"选择文件夹…");self.set(515,"目标目录");}
        if page==3{self.set(RUN,"开始收集");self.set(506,"目标管理员账号");self.set(507,"目标管理员密码");}
        if page==5{self.set(RUN,"开始检测");self.set(SECONDARY,"修复安装连接");}
        if page==7{self.set(513,"超时（分钟）");self.set(RUN,"扫描已安装软件");self.set(SECONDARY,"卸载所选软件");self.set(524,"筛选软件名称、版本或发布者");}
        if page==8{self.set(RUN,"执行日志维护");}
        if page==9{self.set(RUN,"开始诊断收集");self.set(506,"目标管理员账号");self.set(507,"目标管理员密码");}
        if page==10{self.set(RUN,"创建提权快捷方式");self.set(SECONDARY,"预览软件详情");self.set(REMEMBER_CREDENTIALS,"记住此账号密码");self.set(512,"使用者");self.set(SESSIONS,"重新检测用户");}
        else{self.set(512,"登录会话");self.set(SESSIONS,"检测登录会话");}
        if page==4{
            self.set(HINT,"确认后，会把 TransparentEnabled 写到列表中每台电脑上对应目标账号的 HKCU。已有值会被覆盖。");
            self.set(RUN,"检测当前值");self.set(SECONDARY,"确认修改");
        }
        self.configure_columns();
        if page == 0 { self.update_execution_mode(); }
        self.layout();
        if page==11 {self.load_history();} else {self.refresh_results();}
        unsafe{InvalidateRect(self.window,null(),1);}
        if page==6 && self.logs[6].is_empty() {self.append("功能覆盖\r\n已实现：原生窗口、多目标任务、TCP 诊断、WTS 登录会话、管理员会话安装/直接运行、文件/目录分发、基础电脑信息、HKLM/HKU 浏览、TransparentEnabled 检测/写入、结果导出。\r\n未迁移：安装后授权快捷方式、BCU 扫描与卸载、常驻软件提权后台、SSH/WinRM 连接修复、诊断日志打包、Security 日志维护、原版凭据配置与历史数据库。\r\n安装差异：不补建桌面/开始菜单快捷方式、不做回滚、暂按安装器主进程退出码判断完成；目标账号 HKCU 必须已加载。直接运行仅确认进程状态，不承诺窗口可见。\r\n文件分发：不跟随联接/符号链接，不删除目标额外文件；仅支持独立测试目录。".into());}
    }
    fn append(&mut self,text:String) {
        self.append_on(self.page as usize, text);
    }
    fn configure_columns(&self){unsafe{
        while SendMessageW(self.handle(RESULT_TABLE),LVM_DELETECOLUMN,0,0)!=0{}
        let columns=tables::columns(self.page);
        for (index,(title,width)) in columns.iter().enumerate(){
            let mut title=wide(title);let column=LVCOLUMNW{mask:LVCF_TEXT|LVCF_WIDTH,cx:(*width as f64*self.scale) as i32,pszText:title.as_mut_ptr(),..zeroed()};
            SendMessageW(self.handle(RESULT_TABLE),LVM_INSERTCOLUMNW,index,(&column as *const LVCOLUMNW) as isize);
        }
        tables::header(self.handle(RESULT_TABLE),columns.len(),self.page_state.sort[self.page as usize]);
    }}
    fn table_row(&self,index:usize,columns:&[String]){
        unsafe{
            for (column,value) in columns.iter().enumerate(){
                let mut value=wide(value);
                let item=LVITEMW{mask:LVIF_TEXT,iItem:index as i32,iSubItem:column as i32,pszText:value.as_mut_ptr(),..zeroed()};
                SendMessageW(self.handle(RESULT_TABLE),if column==0{LVM_INSERTITEMW}else{LVM_SETITEMW},0,(&item as *const LVITEMW) as isize);
            }
        }
    }
    fn refresh_results(&self){
        let _refresh=tables::Refresh::begin(self.handle(RESULT_TABLE));
        let selected=unsafe{SendMessageW(self.handle(RESULT_TABLE),LVM_GETNEXTITEM,usize::MAX,LVNI_SELECTED as isize)};
        unsafe{SendMessageW(self.handle(RESULT_TABLE),LVM_DELETEALLITEMS,0,0);}
        for (index,result) in self.shown_records().iter().enumerate(){
            self.table_row(index,&tables::row(self.page,result));
        }
        if selected>=0{unsafe{
            let item=LVITEMW{stateMask:LVIS_SELECTED|LVIS_FOCUSED,state:LVIS_SELECTED|LVIS_FOCUSED,..zeroed()};
            SendMessageW(self.handle(RESULT_TABLE),LVM_SETITEMSTATE,selected as usize,(&item as *const LVITEMW) as isize);
        }}
    }
    fn shown_records(&self)->Vec<&serde_json::Value>{
        let query=if self.page==7{self.text(SOFTWARE_SEARCH).to_lowercase()}else{String::new()};
        tables::shown_records(self.page,&self.records[self.page as usize],&query,self.page_state.sort[self.page as usize])
    }
    fn sort_results(&mut self,column:usize){
        let selected=if self.page==11{None}else{self.selected_record()};
        let sort=&mut self.page_state.sort[self.page as usize];let ascending=sort.is_none_or(|(previous,ascending)|previous!=column||!ascending);*sort=Some((column,ascending));
        if self.page==11{self.load_history();}else{self.refresh_results();
            if let Some(selected)=selected{
                if let Some(index)=self.shown_records().iter().position(|row|row["computer"]==selected["computer"]&&row["id"]==selected["id"]&&row["data"]["id"]==selected["data"]["id"]){unsafe{
                    let item=LVITEMW{stateMask:LVIS_SELECTED|LVIS_FOCUSED,state:LVIS_SELECTED|LVIS_FOCUSED,..zeroed()};
                    SendMessageW(self.handle(RESULT_TABLE),LVM_SETITEMSTATE,usize::MAX,(&LVITEMW{state:0,..item} as *const LVITEMW) as isize);SendMessageW(self.handle(RESULT_TABLE),LVM_SETITEMSTATE,index,(&item as *const LVITEMW) as isize);
                }}
            }
        }
        tables::header(self.handle(RESULT_TABLE),tables::columns(self.page).len(),self.page_state.sort[self.page as usize]);
    }
    fn selected_record(&self)->Option<serde_json::Value>{
        let index=unsafe{SendMessageW(self.handle(RESULT_TABLE),LVM_GETNEXTITEM,usize::MAX,LVNI_SELECTED as isize)};
        if index<0{return None;}
        self.shown_records().get(index as usize).map(|record|(*record).clone())
    }
    fn load_history(&mut self){
        match crate::tasks::list(){
            Ok(entries)=>{
                let selected=unsafe{SendMessageW(self.handle(RESULT_TABLE),LVM_GETNEXTITEM,usize::MAX,LVNI_SELECTED as isize)};
                let selected_id=self.history.get(selected as usize).map(|entry|entry.id.clone());
                let _refresh=tables::Refresh::begin(self.handle(RESULT_TABLE));
                self.history=entries;
                if let Some((column,ascending))=self.page_state.sort[11]{self.history.sort_by_cached_key(|entry|tables::sort_key(tables::history_row(entry).get(column).map(String::as_str).unwrap_or_default()));if !ascending{self.history.reverse();}}
                unsafe{SendMessageW(self.handle(RESULT_TABLE),LVM_DELETEALLITEMS,0,0);}
                for (index,entry) in self.history.iter().enumerate(){
                    self.table_row(index,&tables::history_row(entry));
                }
                drop(_refresh);
                if let Some(index)=selected_id.and_then(|id|self.history.iter().position(|entry|entry.id==id)){unsafe{
                    let item=LVITEMW{stateMask:LVIS_SELECTED|LVIS_FOCUSED,state:LVIS_SELECTED|LVIS_FOCUSED,..zeroed()};
                    SendMessageW(self.handle(RESULT_TABLE),LVM_SETITEMSTATE,usize::MAX,(&LVITEMW{state:0,..item} as *const LVITEMW) as isize);
                    SendMessageW(self.handle(RESULT_TABLE),LVM_SETITEMSTATE,index,(&item as *const LVITEMW) as isize);
                }}
                self.set(STATUS,&format!("共 {} 条任务记录",self.history.len()));
                self.set(871,&format!("{}  ACTIVE",self.history.iter().filter(|entry|matches!(entry.state.as_str(),"Queued"|"Running"|"Cancelling")).count()));
            },
            Err(error)=>self.set(STATUS,&error)
        }
    }
    fn show_selected_result(&mut self){
        let index=unsafe{SendMessageW(self.handle(RESULT_TABLE),LVM_GETNEXTITEM,usize::MAX,LVNI_SELECTED as isize)};
        if index<0{return;}
        let data=if self.page==11 {
            self.history.get(index as usize).and_then(|entry|{
                let mut entry=entry.clone();entry.result=crate::tasks::result(&entry.id).ok().flatten();serde_json::to_value(entry).ok()
            })
        } else {self.selected_record()};
        if let Some(data)=data {
            let text=serde_json::to_string_pretty(&data).unwrap_or_default();self.set(LOG,&text);
            if matches!(self.page,0|1|4|5|8|10|11){
                let (text,note)=if text.chars().count()>1_000_000{(text.chars().take(1_000_000).collect::<String>(),"结果较长，仅显示前 100 万字符；完整结果保存在任务历史中。可选择并复制文本。")}
                    else{(text,"以下为选中项目的结果，可选择并复制文本。关闭此窗口不会修改任务。")};
                if let Err(error)=crate::form::show(self.window,"任务结果详情",note,vec![crate::form::Field::memo(&text)]){self.set(STATUS,&error);}
            }
        }
    }
    fn append_on(&mut self, page: usize, text: String) {
        let log = &mut self.logs[page];
        if log.len()>800_000 {log.clear();}
        log.push_str(&text.replace('\n',"\r\n").replace("\r\r\n","\r\n"));log.push_str("\r\n\r\n");
        if self.page as usize == page { self.refresh_log(); }
    }
    fn refresh_log(&self) {
        let log = &self.logs[self.page as usize];
        self.set(LOG, log);
        unsafe {SendMessageW(self.handle(LOG),EM_SETSEL,log.encode_utf16().count(),-1);SendMessageW(self.handle(LOG),EM_SCROLLCARET,0,0);}
    }
    fn source_control(&self) -> i32 { if self.page == 1 { DISTRIBUTION_SOURCE } else { SOURCE } }
    fn direct_run_selected(&self) -> bool {
        unsafe { SendMessageW(self.handle(EXECUTION_MODE), CB_GETCURSEL, 0, 0) == 1 }
    }
    fn update_execution_mode(&self) {
        self.set(RUN, if self.direct_run_selected() { "开始运行" } else { "开始安装" });
    }
    fn browse_remote(&self,control:i32,folder:bool,source:bool){
        let outcome=(||->Result<Option<String>>{
            let host=if source{self.text(SOURCE_COMPUTER).trim().to_owned()}else{
                let hosts=model::hosts(&self.text(TARGET))?;
                if hosts.len()!=1{return Err("浏览文件时请填写一台目标电脑。".into());}hosts[0].clone()
            };
            let account=self.text(if source{SOURCE_ACCOUNT}else{ACCOUNT}).trim().to_owned();
            let mut password=self.text(if source{SOURCE_PASSWORD}else{PASSWORD});
            if password.is_empty(){password=crate::credentials::read(&account).unwrap_or_default();}
            if account.is_empty()||password.is_empty(){return Err("请输入读取该电脑文件所需的管理员账号和密码，或选择已保存凭据。".into());}
            use crate::remote_browser::Mode;
            let mode=if folder{Mode::Folder}else if source{Mode::AnyFile}else if control==ELEVATION_PATH{Mode::Elevation}else if self.direct_run_selected(){Mode::Executable}else{Mode::Installer};
            crate::remote_browser::choose_remote(self.window,&host,Credentials{account,password},&self.text(control),mode)
        })();
        match outcome{Ok(Some(path))=>{self.set(control,&path);self.set(STATUS,"已选择远程电脑上的路径。");},Ok(None)=>(),Err(error)=>self.set(STATUS,&error)}
    }
    fn browse_folder(&self){
        if self.busy{return;}
        if self.checked(REMOTE_SOURCE){self.browse_remote(DISTRIBUTION_SOURCE,true,true);}
        else if let Some(path)=select_local_folder(self.window){self.set(DISTRIBUTION_SOURCE,&path);}
    }
    fn browse(&mut self) {
        if self.busy{return;}
        if self.page==0&&self.checked(EXISTING){self.browse_remote(SOURCE,false,false);return;}
        if self.page==1&&self.checked(REMOTE_SOURCE){self.browse_remote(DISTRIBUTION_SOURCE,false,true);return;}
        if let Some(path)=dialog(self.window,false,"选择安装文件或待分发文件",if self.page==0&&self.direct_run_selected(){"可执行文件\0*.exe\0\0"}else if self.page==0{"安装程序\0*.exe;*.msi\0所有文件\0*.*\0\0"}else{"所有文件\0*.*\0\0"}){self.set(self.source_control(),&path);}
    }
    fn export(&mut self) {
        if let Some(path)=dialog(self.window,true,"保存任务结果","文本文件\0*.txt\0所有文件\0*.*\0\0") {
            match fs::write(&path,&self.logs[self.page as usize]) {Ok(())=>self.set(STATUS,&format!("结果已保存：{path}")),Err(e)=>win::message("导出失败",&e.to_string())}
        }
    }
    fn export_inventory(&self) {
        self.export_inventory_as(None)
    }
    fn export_inventory_as(&self,format:Option<u32>){
        if self.inventory.is_empty() {
            self.set(STATUS, "暂无可导出的电脑信息。");
            return;
        }
        if !self.page_state.export_scope.iter().any(|selected|*selected){self.set(STATUS,"请先选择至少一项导出内容。");return;}
        let filter=if let Some(format)=format{match format{1=>"JSON 文件\0*.json\0\0",2=>"CSV 文件\0*.csv\0\0",_=>"Excel 工作簿\0*.xlsx\0\0"}}else{unsafe {
            let menu=CreatePopupMenu();
            for (index,name) in ["JSON","CSV","Excel 工作簿"].iter().enumerate() {
                AppendMenuW(menu,MF_STRING,index+1,wide(name).as_ptr());
            }
            let mut point=zeroed();GetCursorPos(&mut point);
            let chosen=TrackPopupMenu(menu,TPM_RETURNCMD,point.x,point.y,0,self.window,null());
            DestroyMenu(menu);
            match chosen {1=>"JSON 文件\0*.json\0\0",2=>"CSV 文件\0*.csv\0\0",3=>"Excel 工作簿\0*.xlsx\0\0",_=>return}
        }};
        if let Some(path) = dialog(self.window, true, "导出选中的电脑信息类别", filter) {
            let path = std::path::PathBuf::from(path);
            let groups=[
                &["system","operatingSystem","bios","baseboard","processors","memoryModules","videoControllers"][..],
                &["installedSoftware"][..],&["networkAdapters"][..],&["physicalDisks","logicalDisks"][..],
                &["sessions"][..],&["hotfixes"][..],&["services"][..],&["warnings"][..]
            ];
            let selected:Vec<String>=groups.iter().enumerate().filter(|(index,_)|self.page_state.export_scope[*index])
                .flat_map(|(_,names)|names.iter().map(|name|name.to_string())).collect();
            let format=if filter.starts_with("JSON"){"json"}else if filter.starts_with("CSV"){"csv"}else{"xlsx"};
            let result = crate::export::inventory_format(&path,&self.inventory,&selected,format);
            match result {
                Ok(()) => self.set(STATUS, &format!("已导出 {} 台电脑的结果。", self.inventory.len())),
                Err(error) => self.set(STATUS, &error),
            }
        }
    }
    fn reload_accounts(&self, selected: &str) {
        if let Ok(accounts) = crate::credentials::accounts() {
            unsafe {
                SendMessageW(self.handle(ACCOUNT), CB_RESETCONTENT, 0, 0);
                for account in &accounts {
                    SendMessageW(self.handle(ACCOUNT), CB_ADDSTRING, 0, wide(account).as_ptr() as isize);
                }
            }
            self.set(ACCOUNT, selected);
        }
    }
    fn save_account(&self) {
        let credentials = Credentials { account: self.text(ACCOUNT).trim().into(), password: self.text(PASSWORD) };
        if credentials.account.is_empty() || credentials.password.is_empty() {
            self.set(STATUS, "请输入要保存的账号和密码。");
            return;
        }
        match crate::credentials::write(&credentials) {
            Ok(()) => {
                let _=crate::credentials::remember(&credentials.account);
                self.reload_accounts(&credentials.account);
                self.set(PASSWORD, &credentials.password);
                self.set(STATUS, "凭据已保存到 Windows 凭据管理器。");
            },
            Err(error) => self.set(STATUS, &error),
        }
    }
    fn delete_account(&self) {
        let account = self.text(ACCOUNT);
        match crate::credentials::delete(account.trim()) {
            Ok(()) => { self.reload_accounts(""); self.set(PASSWORD, ""); self.set(STATUS, "已删除保存的凭据。"); },
            Err(error) => self.set(STATUS, &error),
        }
    }
    fn show_sessions(&mut self, computer: String, rows: serde_json::Value, selected: Option<u32>) {
        if model::hosts(&self.text(TARGET)).ok().is_none_or(|targets|!targets.iter().any(|host|host.eq_ignore_ascii_case(&computer))){return;}
        let previous = unsafe { SendMessageW(self.handle(SESSION), CB_GETCURSEL, 0, 0) };
        let selected = selected.or_else(|| self.sessions.get(previous as usize)
            .filter(|(host, _, _)| host.eq_ignore_ascii_case(&computer))
            .map(|(_, _, id)| *id));
        self.sessions.retain(|(host,_,_)|!host.eq_ignore_ascii_case(&computer));
        unsafe{SendMessageW(self.handle(SESSION),CB_RESETCONTENT,0,0);}
        for (_,label,_) in &self.sessions{unsafe{SendMessageW(self.handle(SESSION),CB_ADDSTRING,0,wide(label).as_ptr() as isize);}}
        for row in rows.as_array().into_iter().flatten().filter(|row| row["state"] == "Active") {
            if let (Some(id), Some(account)) = (
                row["sessionId"].as_u64().and_then(|id| u32::try_from(id).ok()).filter(|id| *id != 0),
                row["account"].as_str().filter(|account| !account.trim().is_empty()),
            ) {
                let label = format!("{computer} · {account} · 会话 {id}");
                self.sessions.push((computer.clone(), label.clone(), id));
                unsafe { SendMessageW(self.handle(SESSION), CB_ADDSTRING, 0, wide(&label).as_ptr() as isize); }
            }
        }
        unsafe {
            let index = selected.and_then(|id| self.sessions.iter().position(|(host, _, session)| host.eq_ignore_ascii_case(&computer) && *session == id));
            SendMessageW(self.handle(SESSION), CB_SETCURSEL, index.unwrap_or(if self.sessions.len() == 1 { 0 } else { usize::MAX }), 0);
        }
    }
    fn review_junk(&mut self){
        if self.busy{return;}
        if self.junk_items.is_empty(){self.set(STATUS,"没有待审核的卸载残留。请先卸载选中的软件。");return;}
        match crate::bcu_selection::select(self.window,&self.junk_host,&self.junk_items){
            Ok(Some(selected)) if !selected.is_empty()=>{
                self.junk_selection=selected;self.launch(Some("bcu-delete-junk"));
            },
            Ok(_)=>self.append("已保留卸载残留，未执行清理。".into()),
            Err(error)=>self.set(STATUS,&error),
        }
    }
    fn approve_preview(&mut self){
        let Some(preview)=self.elevation_preview.as_ref()else{return;};
        let Some(candidate)=preview["data"]["candidates"].as_array().and_then(|items|items.first())else{return;};
        if !self.checked(ELEVATION_TRUST){self.append("已完成预览。核对目标和运行链后，请勾选可信软件确认，再创建授权。".into());return;}
        let text=format!("为 {} 创建授权启动入口？\n\n软件：{}\n目标：{}\n参数：{}\nSHA-256：{}\n\n后台将使用指定管理员身份，只允许预览中的用户启动这个固定目标。",
            preview["data"]["userAccount"].as_str().unwrap_or_default(),candidate["name"].as_str().unwrap_or_default(),candidate["target"].as_str().unwrap_or_default(),candidate["arguments"].as_str().unwrap_or_default(),candidate["targetSha256"].as_str().unwrap_or_default());
        if unsafe{MessageBoxW(self.window,wide(&text).as_ptr(),wide("核对软件授权").as_ptr(),MB_YESNO|MB_ICONWARNING|MB_DEFBUTTON2)}==IDYES{self.launch(Some("elevation-approve"));}
    }
    fn retry_selected(&mut self){
        if self.busy{return;}
        let index=unsafe{SendMessageW(self.handle(RESULT_TABLE),LVM_GETNEXTITEM,usize::MAX,LVNI_SELECTED as isize)};
        let Some(entry)=self.history.get(index as usize)else{self.set(STATUS,"请选择需要重试的历史任务。");return;};
        if ["Running","Queued","Cancelling"].contains(&entry.state.as_str()){self.set(STATUS,"所选任务尚未结束。");return;}
        self.retry_request=Some((vec![entry.target.clone()],entry.request.clone()));
        self.launch(Some("retry-history"));
    }
    fn cancel_current(&mut self){
        if self.page==11 {
            let index=unsafe{SendMessageW(self.handle(RESULT_TABLE),LVM_GETNEXTITEM,usize::MAX,LVNI_SELECTED as isize)};
            let result=self.history.get(index as usize).ok_or("请选择正在运行的任务。".to_string()).and_then(|entry|crate::tasks::cancel_task(&entry.id));
            self.set(STATUS,match &result{Ok(())=>"已请求取消选中的任务。",Err(error)=>error});
        }else if let Some(batch)=self.batches.get(&(self.page as usize)){
            crate::tasks::cancel_batch(&batch.id,&batch.cancel);
            self.set(STATUS,"正在取消当前页任务，已完成的操作保留。");
        }
    }
    fn launch(&mut self,override_operation:Option<&str>) {
        if self.page==11 && override_operation.is_none(){self.load_history();return;}
        if self.busy {return;}
        let retried=self.retry_request.is_some();
        let operation=if let Some((_,request))=&self.retry_request {request.operation.clone()}else{override_operation.unwrap_or(match self.page{0 if self.direct_run_selected()=>"run",0=>"install",1=>"distribute",2=>"sessions",3=>"inventory",4=>"registry-query",5=>"diagnose",7=>"bcu-scan",8=>"security-clear",9=>"diagnostic-logs",10=>"elevation-create",_=>return}).to_owned()};
        let result=(||->Result<_>{
            if let Some((target,mut request))=self.retry_request.take(){
                request.id=model::new_id();
                let password=if operation=="diagnose"{String::new()}else{self.retry_password(&request.account)?};
                let credentials=Credentials{account:request.account.clone(),password};
                if operation!="diagnose"{request.validate()?;}
                return Ok((target,credentials,request));
            }
            let mut targets=model::hosts(&self.text(TARGET))?;
            if operation=="connection-repair"{targets=vec![self.selected_record().and_then(|row|row["computer"].as_str().map(str::to_owned)).ok_or("先检测并选中一台需要修复的电脑。")?];}
            if matches!(operation.as_str(),"bcu-scan"|"bcu-uninstall")&&targets.len()!=1{return Err("软件卸载需要单台目标电脑。".into());}
            if operation=="distribute"&&self.checked(REMOTE_SOURCE)&&(self.text(SOURCE_COMPUTER).trim().is_empty()||self.text(SOURCE_ACCOUNT).trim().is_empty()){
                return Err("请输入远程源电脑及其读取账号。".into());
            }
            let mut credentials=Credentials {account:self.text(ACCOUNT).trim().into(),password:self.text(PASSWORD)};
            if operation!="diagnose" && credentials.password.is_empty() && !credentials.account.is_empty() {
                credentials.password = crate::credentials::read(&credentials.account).map_err(|_| "请输入密码，或选择已保存的账号。".to_string())?;
            }
            let session_index = unsafe { SendMessageW(self.handle(SESSION), CB_GETCURSEL, 0, 0) };
            let session = self.sessions.get(session_index as usize)
                .filter(|(computer, _, _)| targets.len() == 1 && computer.eq_ignore_ascii_case(&targets[0]))
                .map_or(0, |(_, _, session)| *session);
            let mut request=Request {id:model::new_id(),operation:operation.clone(),account:credentials.account.clone(),source:self.text(self.source_control()).trim_matches('"').trim().into(),
                destination:self.text(DESTINATION).trim().into(),arguments:self.text(ARGUMENTS),sha256:String::new(),session,
                timeout:self.text(TIMEOUT).trim().parse::<u32>().ok().and_then(|value|value.checked_mul(if self.page==5{1}else{60})).ok_or("超时请输入有效的正整数。")?,existing:self.checked(EXISTING),key:self.text(KEY).trim().into(),value:self.text(VALUE),
                conflict: {
                    use model::ConflictPolicy::*;
                    let index = unsafe { SendMessageW(self.handle(OVERWRITE), CB_GETCURSEL, 0, 0) };
                    [Ask, Overwrite, Skip, Newer, DifferentHash].get(index as usize).copied().unwrap_or(Stop)
                },
                registry_view: if unsafe { SendMessageW(self.handle(REGISTRY_VIEW), CB_GETCURSEL, 0, 0) } == 1 {32} else {64},
                logs: crate::logs::Options {
                    days:[1,3,7,30].get(unsafe{SendMessageW(self.handle(LOG_RANGE),CB_GETCURSEL,0,0)} as usize).copied().unwrap_or(7),
                    performance:self.checked(LOG_OPTIONS),blue_screen:self.checked(LOG_OPTIONS+1),
                    crash:self.checked(LOG_OPTIONS+2),installation:self.checked(LOG_OPTIONS+3),
                    events:self.checked(LOG_OPTIONS+4),reliability:self.checked(LOG_OPTIONS+5),
                    evtx:self.checked(LOG_OPTIONS+6),dumps:self.checked(LOG_OPTIONS+7),security:self.checked(LOG_OPTIONS+8)
                },
                elevation:Default::default(),
                bcu:Default::default(),
                distribution:self.distribution_options(),
                install:self.install_options(),
                inventory: crate::inventory::Selection {
                    system:self.checked(INVENTORY_OPTIONS), hardware:self.checked(INVENTORY_OPTIONS+1),
                    storage:self.checked(INVENTORY_OPTIONS+2), network:self.checked(INVENTORY_OPTIONS+3),
                    sessions:self.checked(INVENTORY_OPTIONS+4),software:self.checked(INVENTORY_OPTIONS+5),
                    hotfixes:self.checked(INVENTORY_OPTIONS+6),services:self.checked(INVENTORY_OPTIONS+7)
                }};
            if operation.starts_with("elevation-"){
                request.source=self.text(ELEVATION_PATH).trim_matches('"').trim().into();
                request.elevation.trusted=operation=="elevation-create"||(operation=="elevation-approve"&&self.elevation_preview.as_ref().is_some_and(|preview|preview["creationRequested"]==true));
                request.elevation.update_identity=self.checked(ELEVATION_UPDATE);
                request.elevation.name_filter=self.text(ELEVATION_FILTER).trim().into();
                if operation=="elevation-create"{
                    let index=unsafe{SendMessageW(self.handle(ELEVATION_CANDIDATE),CB_GETCURSEL,0,0)};
                    if let Some(preview)=self.elevation_preview.as_ref().filter(|preview|targets.len()==1&&preview["computer"].as_str().is_some_and(|host|host.eq_ignore_ascii_case(&targets[0]))){
                        if let Some(candidate)=preview["data"]["candidates"].as_array().and_then(|items|items.get(index as usize)).filter(|candidate|candidate["target"].as_str().is_some_and(|target|target.eq_ignore_ascii_case(&request.source))){
                            request.elevation.candidate=Some(serde_json::from_value(candidate.clone()).map_err(|e|e.to_string())?);
                        }
                    }
                }
                if operation=="elevation-approve"{
                    let preview=self.elevation_preview.as_ref().ok_or("请先预览软件目标。")?;
                    if targets.len()!=1||preview["computer"].as_str().is_none_or(|host|!host.eq_ignore_ascii_case(&targets[0])){return Err("授权预览的目标电脑已改变，请重新预览。".into());}
                    request.elevation.candidate=Some(serde_json::from_value(preview["data"]["candidates"][0].clone()).map_err(|e|e.to_string())?);
                    request.elevation.user_account=preview["data"]["userAccount"].as_str().unwrap_or_default().into();
                    request.elevation.user_sid=preview["data"]["userSid"].as_str().unwrap_or_default().into();
                }
            }
            if operation=="bcu-delete-junk" { if targets.len()!=1||!targets[0].eq_ignore_ascii_case(&self.junk_host){return Err("残留清理目标必须与卸载目标一致。".into());} request.bcu.selected_junk=self.junk_selection.clone(); }
            if operation=="bcu-uninstall"||operation=="elevation-revoke" {
                let selected=self.selected_record().ok_or("请从结果表中选择一个项目。")?;
                if targets.len()!=1||selected["computer"].as_str().is_none_or(|host|!host.eq_ignore_ascii_case(&targets[0])){
                    return Err("此操作需要单台目标，并且选中项目属于该目标。".into());
                }
                if operation=="bcu-uninstall"{
                    if selected["data"]["protected"]==true||selected["data"]["valid"]==false||selected["data"]["interactive"]!=true{return Err("所选软件受保护、缺少普通卸载命令或卸载程序无效。".into());}
                    request.bcu.selected_entry=selected["data"]["entryXml"].as_str().ok_or("选中项目不支持卸载。")?.into();
                    request.bcu.display_name=selected["data"]["displayName"].as_str().unwrap_or_default().into();
                }else{request.elevation.rule_id=selected["data"]["id"].as_str().ok_or("请选择授权规则。")?.into();}
            }
            if operation=="connection-repair"{request.timeout=90;}
            if operation!="diagnose" {request.validate()?;if credentials.password.is_empty(){return Err("请输入本次任务使用的密码。".into());}}
            else if !(1..=30).contains(&request.timeout){return Err("检测超时请输入 1～30 秒。".into());}
            Ok((targets,credentials,request))
        })();
        let (targets,credentials,request)=match result {
            Ok(value) => value,
            Err(error) => {
                self.set(STATUS, &error);
                if operation == "bcu-uninstall" {
                    unsafe { MessageBoxW(self.window, wide(&error).as_ptr(), wide("无法启动卸载").as_ptr(), MB_OK | MB_ICONINFORMATION); }
                }
                return;
            }
        };
        let source_credentials=match self.source_credentials(&request,&credentials){Ok(credentials)=>credentials,Err(error)=>{self.set(STATUS,&error);return;}};
        if operation=="distribute"&&(request.distribution.destination_preset=="Custom"&&crate::distribution::high_risk(&request.destination)||request.distribution.destination_preset!="Custom"&&request.distribution.destination_preset!="ProgramDataCompany"){
            let destination=if request.distribution.destination_preset=="Custom"{request.destination.clone()}else{format!("预设：{}（每台目标机按实际用户及系统配置解析目录）",self.text(DESTINATION_PRESET))};
            if unsafe{MessageBoxW(self.window,wide(&format!("目标：{destination}\n确认向 {} 写入文件？\n不会执行文件或删除目标的其他内容；覆盖同名文件可能影响系统、应用或用户配置。",targets.join(", "))).as_ptr(),wide("核对分发目标").as_ptr(),MB_YESNO|MB_ICONWARNING|MB_DEFBUTTON2)}!=IDYES{return;}
        }
        if matches!(operation.as_str(),"registry-set"|"security-clear"|"bcu-uninstall"|"bcu-delete-junk"|"elevation-revoke"|"connection-repair")||(operation=="elevation-approve"&&retried) {
            let text=if operation=="registry-set"{format!("将 {} 台目标电脑上账号 {} 的 TransparentEnabled 写为 {}。\n确认执行？",targets.len(),request.account,request.value)}
                else if operation=="security-clear"{format!("清理 {} 台电脑的安全日志并启用按需覆盖。确认执行？",targets.len())}
                else if operation=="bcu-uninstall"{format!("卸载目标软件 {}？",request.bcu.display_name)}
                else if operation=="bcu-delete-junk"{format!("在 {} 上清理选中的 {} 个残留项？\n文件和文件夹请求移至回收站；注册表项将直接删除。",targets.join(", "),request.bcu.selected_junk.len())}
                else if operation=="elevation-approve"{format!("在 {} 上创建已预览软件的授权入口？",targets.join(", "))}
                else if operation=="connection-repair"{format!("修复 {} 台目标电脑的域网络共享规则并启动 LanmanServer、fdPHost、FDResPub 服务？",targets.len())}
                else{"撤销选中的软件授权？已运行的软件将继续运行。".into()};
            if unsafe{MessageBoxW(self.window,wide(&text).as_ptr(),wide("确认操作").as_ptr(),MB_YESNO|MB_ICONQUESTION|MB_DEFBUTTON2)}!=IDYES{return;}
        }
        if (operation!="registry-query"&&operation!="diagnose")&&self.checked(REMEMBER_CREDENTIALS){
            if let Err(error)=crate::credentials::write(&credentials){self.set(STATUS,&error);return;}
        }
        if operation=="distribute"&&self.checked(REMEMBER_SOURCE){
            if let Some(credentials)=&source_credentials{if let Err(error)=crate::credentials::write(credentials){self.set(STATUS,&error);return;}}
        }
        if operation=="distribute"{self.page_state.last_distribution=Some(request.clone());}
        if operation=="bcu-uninstall" {self.junk_items.clear();self.review_pending=false;}
        if self.page == 10 && matches!(operation.as_str(), "elevation-create" | "elevation-approve") {
            self.set(ELEVATION_RECEIPT, "");
            self.layout();
        }
        if operation == "sessions" {
            self.sessions.clear();
            unsafe { SendMessageW(self.handle(SESSION), CB_RESETCONTENT, 0, 0); }
        }
        if operation == "inventory" { self.inventory.clear(); }
        if !matches!(operation.as_str(),"sessions"|"bcu-uninstall"|"bcu-delete-junk"|"connection-repair"|"elevation-preview"|"elevation-discover"|"elevation-create"|"elevation-approve"|"elevation-revoke"){self.records[self.page as usize].clear();self.refresh_results();}
        self.busy=true;self.cancel=Arc::new(AtomicBool::new(false));self.started=Instant::now();
        let page=self.page as usize;let batch_id=model::new_id();
        self.batches.insert(page,Batch{id:batch_id.clone(),cancel:self.cancel.clone(),started:self.started});
        self.set(STATUS,"任务正在后台执行…");
        unsafe{for id in [RUN,SECONDARY,THIRD,SESSIONS]{EnableWindow(self.handle(id),0);}EnableWindow(self.handle(CANCEL),1);}
        self.append(format!("开始任务：{operation}，共 {} 台电脑。",targets.len()));
        let hwnd=self.window as usize;let cancel=self.cancel.clone();
        let concurrency = if operation=="distribute"{request.distribution.target_concurrency}else if operation=="diagnostic-logs"{
            [1,2,4,8].get(unsafe{SendMessageW(self.handle(LOG_CONCURRENCY),CB_GETCURSEL,0,0)} as usize).copied().unwrap_or(4)
        }else if operation == "inventory" {
            let index = unsafe { SendMessageW(self.handle(INVENTORY_CONCURRENCY), CB_GETCURSEL, 0, 0) };
            [1, 2, 4, 8, 12, 16].get(index as usize).copied().unwrap_or(4)
        } else {match operation.as_str(){"diagnose"=>8,"security-clear"|"security-info"=>2,_=>4}};
        std::thread::spawn(move||{
            let report:Reporter=Arc::new(move|line|post_on(hwnd,WM_LOG,page,line));
            let selected_sessions=match session_selection::select(hwnd,&targets,&credentials,&request,&cancel,&report){
                Ok(selected)=>selected,Err(error)=>{post_on(hwnd,WM_LOG,page,error.clone());post_on(hwnd,WM_DONE,page,error);return;}
            };
            let prepared=targets.iter().map(|computer|{
                let mut request=request.clone();request.id=model::new_id();
                if let Some((session, account)) = selected_sessions.get(computer) {
                    request.session = *session;
                    post_on(hwnd, WM_SESSIONS, page, serde_json::json!([computer, [{"sessionId": session, "account": account, "state": "Active"}], session]).to_string());
                }
                let cancellation=crate::tasks::Cancellation::register(&request.id,&batch_id,&cancel);
                let queued=crate::tasks::enqueue(&batch_id,computer,&request);
                post_on(hwnd,WM_PROGRESS,page,serde_json::json!({"id":request.id,"computer":computer,"state":"等待执行","operation":operation,"detail":"正在排队"}).to_string());
                (request,cancellation,queued)
            }).collect::<Vec<_>>();
            use std::sync::atomic::AtomicUsize;
            let next = AtomicUsize::new(0);
            let results = std::sync::Mutex::new(vec![None; targets.len()]);
            std::thread::scope(|scope| {
              for _ in 0..concurrency.min(targets.len()) {
                scope.spawn(|| loop {
                if cancel.load(Ordering::Relaxed){break;}
                let index = next.fetch_add(1, Ordering::Relaxed);
                let Some(computer) = targets.get(index) else { break; };
                report(format!("[{computer}] 开始处理"));
                let (request,cancellation,queued)=&prepared[index];
                let start=Instant::now();
                let outcome = (|| {
                    queued.as_ref().map_err(|error|error.clone())?;
                    let cancellation=cancellation.as_ref().map_err(|error|error.clone())?;
                    let task_cancel=&*cancellation.flag;
                    let _lease=if matches!(operation.as_str(),"install"|"run"|"distribute"|"registry-set"|"security-clear"|"connection-repair"|"bcu-uninstall"|"bcu-delete-junk"|"elevation-create"|"elevation-approve"|"elevation-revoke"){
                        Some(crate::tasks::TargetLease::acquire(computer,task_cancel)?)
                    }else{None};
                    crate::tasks::update(&request.id, "Running", "正在执行", None)?;
                    let progress=|data:serde_json::Value|{
                        if let Some(session) = data.get("interactiveSession") {
                            post_on(hwnd, WM_SESSIONS, page, serde_json::json!([computer, [session], session["sessionId"]]).to_string());
                            return;
                        }
                        let _=crate::tasks::progress(&request.id,data["progress"].as_u64().unwrap_or_default() as u32,data["detail"].as_str().unwrap_or_default());
                        post_on(hwnd,WM_PROGRESS,page,serde_json::json!({"id":request.id,"computer":computer,"state":"执行中","operation":operation,"detail":data["detail"],"data":data}).to_string());
                    };
                    if operation=="diagnose" { crate::diagnostics::diagnose(computer,request.timeout,&credentials,&report,task_cancel) }
                    else { remote::execute_with_source(computer,&credentials,source_credentials.as_ref(),request,&report,task_cancel,Some(&progress)) }
                })();
                let persisted = match &outcome {
                    Ok(data) => crate::tasks::update(&request.id, model::outcome_state(data), &model::outcome_detail(data), Some(data)),
                    Err(error) => crate::tasks::update(&request.id, if cancel.load(Ordering::Relaxed)||cancellation.as_ref().is_ok_and(|task|task.flag.load(Ordering::Relaxed)) {"Cancelled"} else {"Failed"},error,None),
                };
                if let Err(error) = persisted { report(format!("任务状态保存失败：{error}")); }
                let table_record=match &outcome{
                    Ok(data)=>serde_json::json!({"id":request.id,"computer":computer,"state":model::outcome_label(data),"operation":operation,
                        "detail":format!("{} · {:.1} 秒",model::outcome_detail(data),start.elapsed().as_secs_f64()),"data":data}),
                    Err(error)=>serde_json::json!({"id":request.id,"computer":computer,"state":"失败 / 未完成","operation":operation,"detail":error})
                };
                post_on(hwnd,WM_RESULT,page,table_record.to_string());
                let (state, label, detail) = match &outcome {
                    Ok(data) => {
                        let detail = model::outcome_detail(data);
                        (model::outcome_state(data), model::outcome_label(data),
                            (model::outcome_state(data) != "Succeeded" || detail != "已收到目标结果").then_some(detail))
                    }
                    Err(error) if cancel.load(Ordering::Relaxed) || cancellation.as_ref().is_ok_and(|task| task.flag.load(Ordering::Relaxed)) =>
                        ("Cancelled", "已取消", Some(error.clone())),
                    Err(error) => ("Failed", "失败", Some(error.clone())),
                };
                let line = format!("{computer}：{label} · {:.1} 秒{}", start.elapsed().as_secs_f64(),
                    detail.as_ref().map(|reason| format!(" · {reason}")).unwrap_or_default());
                results.lock().unwrap()[index] = Some((state, line.clone()));
                if operation == "inventory" {
                    if let Ok(data) = &outcome {
                        if let Some(sessions) = data["sessions"].as_array() {
                            post_on(hwnd, WM_SESSIONS, page, serde_json::json!([computer, sessions, null]).to_string());
                        }
                    }
                    let item = match &outcome {
                        Ok(data) => serde_json::json!({"computer": computer, "success": true, "data": data}),
                        Err(error) => serde_json::json!({"computer": computer, "success": false, "error": error}),
                    };
                    post_on(hwnd, WM_INVENTORY,page,item.to_string());
                }
                match outcome {
                    Ok(data)=>{
                        if operation == "sessions" {
                            post_on(hwnd, WM_SESSIONS,page,serde_json::json!([computer, data,null]).to_string());
                        }
                        if operation == "bcu-scan" {
                            let sessions = &data["sessions"];
                            if sessions.is_array() {
                                post_on(hwnd, WM_SESSIONS, page, serde_json::json!([computer, sessions, null]).to_string());
                            } else {
                                post_on(hwnd, WM_SESSIONS, page, serde_json::json!([computer, [], null]).to_string());
                            }
                            if let Some(error) = data["sessionWarning"].as_str() {
                                report(format!("{computer}：软件扫描完成，但登录会话读取失败。{error}"));
                            } else if !sessions.as_array().is_some_and(|rows| rows.iter().any(|row| row["state"] == "Active" && row["sessionId"].as_u64().is_some_and(|id| id > 0))) {
                                report(format!("{computer}：软件扫描完成，目标电脑当前没有活动登录会话。"));
                            }
                        }
                        report(format!("[{computer}] {label}，耗时 {} ms\r\n{}",start.elapsed().as_millis(),serde_json::to_string_pretty(&data).unwrap_or_default()));
                        if detail.is_some() { report(line); }
                    },
                    Err(_) => report(line),
                }
                });
              }
            });
            let results=results.into_inner().unwrap();
            for ((request,_,queued),result) in prepared.iter().zip(&results){
                if result.is_none()&&queued.is_ok(){let _=crate::tasks::update(&request.id,"Cancelled","排队任务已取消，未执行。",None);}
            }
            let count = |state| results.iter().flatten().filter(|(outcome, _)| *outcome == state).count();
            let mut counts = vec![format!("成功 {}", count("Succeeded"))];
            for (state, label) in [("PartialSuccess", "部分成功"), ("Failed", "失败"), ("Unconfirmed", "待确认"), ("Cancelled", "已取消")] {
                let n = count(state);
                if n > 0 { counts.push(format!("{label} {n}")); }
            }
            let not_run = results.iter().filter(|item| item.is_none()).count();
            if not_run > 0 { counts.push(format!("未执行 {not_run}")); }
            let summary = format!("任务结束：{}", counts.join("，"));
            let details = targets.iter().zip(results)
                .map(|(computer, result)| result.map(|(_, line)| line).unwrap_or_else(|| format!("{computer}：未执行")))
                .collect::<Vec<_>>().join("\r\n");
            let record = format!("操作：{operation}\r\n目标数：{}\r\n{summary}\r\n\r\n{details}\r\n", targets.len());
            if let Err(error) = save_summary(&record) { report(format!("任务记录保存失败：{error}")); }
            post_on(hwnd,WM_DONE,page,summary);
        });
    }
}

unsafe extern "system" fn navigation_keys(window:HWND,message:u32,w:WPARAM,l:LPARAM,id:usize,other:usize)->LRESULT{unsafe{
    use windows_sys::Win32::UI::Shell::{DefSubclassProc,RemoveWindowSubclass};
    let control=GetDlgCtrlID(window);
    if message==WM_KEYDOWN{
        let selected=SendMessageW(window,LB_GETCURSEL,0,0);let count=SendMessageW(window,LB_GETCOUNT,0,0);
        let next=if control==NAV&&w==0x28&&selected==count-1{Some(0)}else if control==NAV_MAINTENANCE&&w==0x26&&selected<=0{Some(SendMessageW(other as HWND,LB_GETCOUNT,0,0)-1)}else{None};
        if let Some(next)=next{
            let other=other as HWND;SendMessageW(other,LB_SETCURSEL,next as usize,0);
            windows_sys::Win32::UI::Input::KeyboardAndMouse::SetFocus(other);
            SendMessageW(GetParent(window),WM_COMMAND,GetDlgCtrlID(other) as usize|((LBN_SELCHANGE as usize)<<16),other as isize);return 0;
        }
    }
    if message==WM_NCDESTROY{RemoveWindowSubclass(window,Some(navigation_keys),id);}
    DefSubclassProc(window,message,w,l)
}}
fn post_on(window:usize,message:u32,page:usize,text:String) {
    let pointer=Box::into_raw(Box::new(text));
    if unsafe{PostMessageW(window as HWND,message,page,pointer as isize)}==0 {unsafe{drop(Box::from_raw(pointer));}}
}
fn dialog(owner:HWND,save:bool,title:&str,filter:&str)->Option<String> {
    unsafe {
        let mut buffer=vec![0u16;32768];let extension = if filter.contains("*.xlsx") {"xlsx"} else if filter.contains("*.csv") {"csv"} else if filter.contains("*.json") { "json" } else { "txt" };let filter:Vec<u16>=filter.encode_utf16().collect();let title=wide(title);
        let mut ofn:OPENFILENAMEW=zeroed();ofn.lStructSize=size_of::<OPENFILENAMEW>() as u32;ofn.hwndOwner=owner;
        ofn.lpstrFile=buffer.as_mut_ptr();ofn.nMaxFile=buffer.len() as u32;ofn.lpstrFilter=filter.as_ptr();ofn.lpstrTitle=title.as_ptr();
        ofn.Flags=OFN_EXPLORER|OFN_NOCHANGEDIR|OFN_PATHMUSTEXIST|if save{OFN_OVERWRITEPROMPT}else{OFN_FILEMUSTEXIST};
        let ext=wide(extension);if save{ofn.lpstrDefExt=ext.as_ptr();}
        if (if save{GetSaveFileNameW(&mut ofn)}else{GetOpenFileNameW(&mut ofn)})==0{None}else{Some(win::string(buffer.as_ptr()))}
    }
}

unsafe extern "system" fn procedure(window:HWND,msg:u32,w:WPARAM,l:LPARAM)->LRESULT {
    unsafe {
        if msg==WM_NCCREATE {
            let create=&*(l as *const CREATESTRUCTW);let ui=create.lpCreateParams as *mut Ui;
            (*ui).window=window;SetWindowLongPtrW(window,GWLP_USERDATA,ui as isize);
        }
        let ptr=GetWindowLongPtrW(window,GWLP_USERDATA) as *mut Ui;
        if ptr.is_null(){return DefWindowProcW(window,msg,w,l);}
        let ui=&mut *ptr;
        match msg {
            WM_MEASUREITEM=>{
                let item=&mut *(l as *mut MEASUREITEMSTRUCT);
                if item.CtlType==ODT_COMBOBOX{item.itemHeight=(30.0*ui.scale).round() as u32;return 1;}
                DefWindowProcW(window,msg,w,l)
            },
            WM_DRAWITEM=>{
                let draw=&*(l as *const DRAWITEMSTRUCT);
                if draw.CtlType==ODT_COMBOBOX{
                    let selected=draw.itemState&ODS_SELECTED!=0;
                    let disabled=draw.itemState&ODS_DISABLED!=0;
                    let background=CreateSolidBrush(if selected{0x00f2edea}else{0xffffff});
                    FillRect(draw.hDC,&draw.rcItem,background);DeleteObject(background);
                    SetBkMode(draw.hDC,TRANSPARENT as i32);
                    SetTextColor(draw.hDC,if disabled{0x00a7988e}else{0x00443730});
                    if draw.itemID!=u32::MAX{
                        let length=SendMessageW(draw.hwndItem,CB_GETLBTEXTLEN,draw.itemID as usize,0);
                        if length>=0{
                            let mut label=vec![0u16;length as usize+1];
                            SendMessageW(draw.hwndItem,CB_GETLBTEXT,draw.itemID as usize,label.as_mut_ptr() as isize);
                            let mut rect=draw.rcItem;rect.left+=(6.0*ui.scale).round() as i32;
                            DrawTextW(draw.hDC,label.as_ptr(),-1,&mut rect,DT_SINGLELINE|DT_VCENTER|DT_END_ELLIPSIS|DT_NOPREFIX);
                        }
                    } else if draw.CtlID == SESSION as u32 && draw.itemState & ODS_COMBOBOXEDIT != 0 {
                        let hint = if ui.sessions.is_empty() {
                            "尚无活动会话，请检测登录会话"
                        } else {
                            "请选择活动登录会话"
                        };
                        let mut rect = draw.rcItem;
                        rect.left += (6.0 * ui.scale).round() as i32;
                        SetTextColor(draw.hDC, 0x0078736d);
                        DrawTextW(draw.hDC, wide(hint).as_ptr(), -1, &mut rect, DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS | DT_NOPREFIX);
                    }
                    if draw.itemState&ODS_FOCUS!=0{DrawFocusRect(draw.hDC,&draw.rcItem);}
                    return 1;
                }
                if matches!(draw.CtlID,id if id==NAV as u32||id==NAV_MAINTENANCE as u32) {
                    if draw.itemID==u32::MAX{return 1;}
                    let selected=draw.itemState&ODS_SELECTED!=0;
                    let background=CreateSolidBrush(if selected{0x00f2edea}else{0x00faf9f7});
                    FillRect(draw.hDC,&draw.rcItem,background);DeleteObject(background);
                    SetTextColor(draw.hDC,if selected{0x00342a24}else{0x00798870});
                    SetBkMode(draw.hDC,TRANSPARENT as i32);
                    let mut label=vec![0u16;256];SendMessageW(draw.hwndItem,LB_GETTEXT,draw.itemID as usize,label.as_mut_ptr() as isize);
                    let mut rect=draw.rcItem;rect.left+=(14.0*ui.scale) as i32;
                    DrawTextW(draw.hDC,label.as_ptr(),-1,&mut rect,DT_SINGLELINE|DT_VCENTER|DT_END_ELLIPSIS|DT_NOPREFIX);
                    if draw.itemState&ODS_FOCUS!=0 {DrawFocusRect(draw.hDC,&draw.rcItem);}
                    return 1;
                }
                if draw.CtlType==ODT_BUTTON {
                    let enabled=IsWindowEnabled(draw.hwndItem)!=0;
                    let primary=draw.CtlID==RUN as u32&&ui.page!=4;
                    let pressed=draw.itemState&ODS_SELECTED!=0;
                    let background=CreateSolidBrush(if !enabled{0x00f0eeeb}else if primary{if pressed{0x0043342d}else{0x0058463c}}else{0x00ffffff});
                    let border=CreatePen(PS_SOLID,1,if primary{0x0058463c}else{0x00eae5e2});
                    let old_brush=SelectObject(draw.hDC,background);
                    let old_pen=SelectObject(draw.hDC,border);
                    RoundRect(draw.hDC,draw.rcItem.left,draw.rcItem.top,draw.rcItem.right,draw.rcItem.bottom,10,10);
                    SelectObject(draw.hDC,old_brush);SelectObject(draw.hDC,old_pen);DeleteObject(background);DeleteObject(border);
                    SetTextColor(draw.hDC,if !enabled{0x00a7988e}else if primary{0x00ffffff}else{0x00443730});
                    SetBkMode(draw.hDC,TRANSPARENT as i32);
                    let mut label=vec![0u16;GetWindowTextLengthW(draw.hwndItem) as usize+1];
                    GetWindowTextW(draw.hwndItem,label.as_mut_ptr(),label.len() as i32);
                    let mut rect=draw.rcItem;
                    DrawTextW(draw.hDC,label.as_ptr(),-1,&mut rect,DT_SINGLELINE|DT_CENTER|DT_VCENTER|DT_NOPREFIX);
                    if draw.itemState&ODS_FOCUS!=0 {InflateRect(&mut rect,-4,-4);DrawFocusRect(draw.hDC,&rect);}
                    return 1;
                }
                DefWindowProcW(window,msg,w,l)
            },
            WM_ERASEBKGND=>{
                let mut rect=zeroed();GetClientRect(window,&mut rect);
                FillRect(w as HDC,&rect,GetStockObject(WHITE_BRUSH));
                if ui.page==4{
                    let mut body=rect;body.left=(224.0*ui.scale) as i32;body.top=(88.0*ui.scale) as i32;
                    let brush=CreateSolidBrush(0x00f8f7f7);FillRect(w as HDC,&body,brush);DeleteObject(brush);
                }
                rect.right=(205.0*ui.scale) as i32;
                let sidebar=CreateSolidBrush(0x00faf9f7);
                FillRect(w as HDC,&rect,sidebar);DeleteObject(sidebar);
                1
            },
            WM_CTLCOLORSTATIC=>{
                let dc=w as HDC;
                let control=l as HWND;
                if control!=ui.handle(LOG) {
                    let sidebar=GetParent(control)!=ui.canvas&&!matches!(control,handle if handle==ui.handle(TITLE)||handle==ui.handle(529));
                    let dark=ui.page==11&&[828,829,870,871].iter().any(|id|control==ui.handle(*id));
                    SetTextColor(dc,if dark{0xffffff}else{0x00443730});
                    SetBkMode(dc,TRANSPARENT as i32);
                    let color=if dark{0x002d2118}else if sidebar{0x00faf9f7}else if ui.page==4&&GetParent(control)==ui.canvas{canvas::background(ui.page)}else{0xffffff};
                    SetBkColor(dc,color);SetDCBrushColor(dc,color);
                    return GetStockObject(DC_BRUSH) as isize;
                }
                DefWindowProcW(window,msg,w,l)
            },
            WM_CREATE=>{ui.build();SetTimer(window,1,1000,None);0},
            WM_SIZE=>{ui.layout();0},
            canvas::SCROLL_MESSAGE=>{canvas::scroll(ui,w,false);0},
            canvas::WHEEL_MESSAGE=>{canvas::scroll(ui,w,true);0},
            splitter::MOVE=>{splitter::adjust(ui, l as i32, w != 0);0},
            WM_GETMINMAXINFO=>{let info=&mut *(l as *mut MINMAXINFO);info.ptMinTrackSize.x=(1145.0*ui.scale) as i32;info.ptMinTrackSize.y=(730.0*ui.scale) as i32;0},
            WM_COMMAND=>{
                let id=(w&0xffff) as i32;let notification=(w>>16) as u32;
                if matches!(id,NAV|NAV_MAINTENANCE)&&notification==LBN_SELCHANGE as u32 {
                    let index=SendMessageW(ui.handle(id),LB_GETCURSEL,0,0);
                    let items=if id==NAV{&NAVIGATION[..4]}else{&NAVIGATION[5..]};
                    if let Some((page,_))=items.get(index as usize){ui.select_page(*page);}
                }
                else if id == ACCOUNT && notification == CBN_EDITCHANGE {
                    ui.set(PASSWORD, "");
                    ui.invalidate_suggestions();
                }
                else if id == ACCOUNT && notification == CBN_SELENDOK {
                    ui.set(PASSWORD, "");
                    ui.invalidate_suggestions();
                    let account = ui.text(ACCOUNT);
                    if let Ok(password) = crate::credentials::read(account.trim()) {
                        let password = zeroize::Zeroizing::new(password);
                        ui.set(PASSWORD, &password);
                        let _=crate::credentials::remember(account.trim());
                    }
                }
                else if id==TARGET&&notification==EN_CHANGE{
                    ui.sessions.clear();SendMessageW(ui.handle(SESSION),CB_RESETCONTENT,0,0);
                    ui.invalidate_suggestions();
                }
                else if id==ELEVATION_CANDIDATE&&notification==CBN_SELCHANGE{
                    let index=SendMessageW(ui.handle(ELEVATION_CANDIDATE),CB_GETCURSEL,0,0);
                    if let Some(candidate)=ui.elevation_preview.as_ref().and_then(|preview|preview["data"]["candidates"].as_array()).and_then(|items|items.get(index as usize)).cloned(){
                        if let Some(target)=candidate["target"].as_str(){ui.set(ELEVATION_PATH,target);}
                        ui.set(ELEVATION_PREVIEW,&serde_json::to_string_pretty(&candidate).unwrap_or_default());
                    }
                }
                else if matches!(id,SOURCE|DISTRIBUTION_SOURCE)&&notification==CBN_DROPDOWN{ui.load_suggestions();}
                else if matches!(id,SOURCE|DISTRIBUTION_SOURCE)&&notification==CBN_EDITCHANGE{SetTimer(window,2,400,None);}
                else if matches!(id,SOURCE|DISTRIBUTION_SOURCE)&&notification==CBN_SELCHANGE{ui.select_suggestion();}
                else if matches!(id,SOURCE_COMPUTER|SOURCE_ACCOUNT|SOURCE_PASSWORD|PASSWORD)&&notification==EN_CHANGE{ui.invalidate_suggestions();}
                else if id == EXECUTION_MODE && notification == CBN_SELCHANGE {
                    ui.invalidate_suggestions();ui.update_execution_mode();
                }
                else if id==SOFTWARE_SEARCH&&notification==EN_CHANGE{ui.refresh_results();}
                else if (id==DESTINATION_PRESET&&notification==CBN_SELCHANGE)||(matches!(id,LOCAL_SOURCE|REMOTE_SOURCE)&&notification==BN_CLICKED){ui.invalidate_suggestions();ui.layout();}
                else if matches!(id,EXISTING|UPLOAD_PACKAGE)&&notification==BN_CLICKED {
                    SendMessageW(ui.handle(EXISTING),BM_SETCHECK,if id==EXISTING{BST_CHECKED}else{BST_UNCHECKED} as usize,0);
                    SendMessageW(ui.handle(UPLOAD_PACKAGE),BM_SETCHECK,if id==UPLOAD_PACKAGE{BST_CHECKED}else{BST_UNCHECKED} as usize,0);
                    ui.invalidate_suggestions();ui.set(SOURCE, "");
                    ui.set(BROWSE, if ui.page == 0 && ui.checked(EXISTING) { "浏览目标机…" } else { "浏览文件…" });
                }
                else if matches!(id,ROLLBACK_ENABLED|ELEVATED_SHORTCUT)&&notification==BN_CLICKED{
                    if id==ELEVATED_SHORTCUT{
                        let elevated=ui.checked(ELEVATED_SHORTCUT);
                        SendMessageW(ui.handle(DESKTOP_SHORTCUT),BM_SETCHECK,if elevated{BST_UNCHECKED}else{BST_CHECKED} as usize,0);
                        if elevated{SendMessageW(ui.handle(PUBLIC_DESKTOP),BM_SETCHECK,BST_UNCHECKED as usize,0);}
                    }
                    ui.layout();
                }
                else if matches!(id,ELEVATION_ADVANCED|ELEVATION_MANAGE)&&notification==BN_CLICKED{ui.layout();}
                else if notification==BN_CLICKED {
                    match id {
                        RUN=>ui.launch(None),
                        SECONDARY=>match ui.page{
                            1=>ui.browse_folder(),
                            3=>ui.export_inventory(),4=>ui.launch(Some("registry-set")),8=>ui.launch(Some("security-clear")),
                            7=>ui.launch(Some("bcu-uninstall")),10=>ui.launch(Some("elevation-preview")),11=>ui.retry_selected(),
                            5=>ui.launch(Some("connection-repair")),
                            9=>{if let Ok(path)=crate::tasks::data_directory(){let _=std::process::Command::new("explorer.exe").arg(path.join("Diagnostics")).spawn();}},
                            _=>()
                        },
                        TASK_HISTORY=>{
                            let result = history_directory().and_then(|directory| {
                                win::io(fs::create_dir_all(&directory))?;
                                win::io(std::process::Command::new("explorer.exe").arg(directory).spawn()).map(|_| ())
                            });
                            if let Err(error) = result { ui.set(STATUS, &error); }
                        },
                        THIRD=>if ui.page==7{ui.review_junk()}else{ui.launch(Some(if ui.page==10{"elevation-revoke"}else{"registry-set"}))},SESSIONS=>ui.launch(Some("sessions")),BROWSE=>ui.browse(),EXPORT=>ui.export(),
                        SAVE_ACCOUNT=>ui.save_account(),DELETE_ACCOUNT=>ui.delete_account(),
                        CANCEL=>ui.cancel_current(),TASK_CENTER=>ui.select_page(11),FILTERS=>ui.edit_filters(),
                        RETRY_DISTRIBUTION=>ui.retry_distribution(),EXPORT_SCOPE=>ui.export_scope(),
                        EXPORT_JSON=>ui.export_inventory_as(Some(1)),EXPORT_CSV=>ui.export_inventory_as(Some(2)),EXPORT_EXCEL=>ui.export_inventory_as(Some(3)),REPAIR_NETWORK=>ui.select_page(5),
                        BCU_LICENSE=>{let result=crate::bundle::Executable::current().and_then(|mut executable|executable.notice());match result{Ok(text)=>win::message("BCU 组件许可",&text),Err(error)=>ui.set(STATUS,&error)}},
                        CLEAR_DIAGNOSTICS=>{if !ui.busy{ui.records[5].clear();ui.logs[5].clear();ui.refresh_results();ui.refresh_log();ui.set(STATUS,"已清空检测结果。");}},
                        OPEN_BUNDLE=>ui.open_bundle(false),OPEN_BUNDLE_FOLDER=>ui.open_bundle(true),
                        ELEVATION_BROWSE=>ui.browse_remote(ELEVATION_PATH,false,false),
                        ELEVATION_LIST=>ui.launch(Some("elevation-list")),ELEVATION_DISCOVER=>ui.launch(Some("elevation-discover")),
                        CLEAR_TARGETS=>{ui.set(TARGET,"");ui.sessions.clear();SendMessageW(ui.handle(SESSION),CB_RESETCONTENT,0,0);ui.set(STATUS,"目标电脑和会话选择已清空。");},
                        _=>(),
                    }
                }0
            },
            WM_LOG=>{
                let text = *Box::from_raw(l as *mut String);
                if !ui.closing { ui.append_on(w as usize, text); }
                0
            },
            session_selection::MESSAGE=>{
                let prompt=*Box::from_raw(l as *mut session_selection::Prompt);
                session_selection::show(window,prompt,ui.closing);0
            },
            suggestions::MESSAGE=>{let text=*Box::from_raw(l as *mut String);if !ui.closing{ui.show_suggestions(&text);}0},
            WM_SESSIONS=>{
                let text = *Box::from_raw(l as *mut String);
                if !ui.closing {
                    if let Ok((computer, rows, selected)) = serde_json::from_str::<(String, serde_json::Value, Option<u32>)>(&text) {
                        ui.show_sessions(computer, rows, selected);
                    }
                }
                0
            },
            WM_INVENTORY=>{
                let text = *Box::from_raw(l as *mut String);
                if let Ok(item) = serde_json::from_str(&text) { ui.inventory.push(item); }
                0
            },
            WM_RESULT=>{
                let text=*Box::from_raw(l as *mut String);
                if let Ok(data)=serde_json::from_str::<serde_json::Value>(&text){
                    let operation=data["operation"].as_str().unwrap_or_default();
                    if matches!(operation,"elevation-preview"|"elevation-discover")&&data["data"]["candidates"].is_array(){
                        ui.elevation_preview=Some(data.clone());ui.approve_pending=false;
                        ui.set(ELEVATION_PREVIEW,&serde_json::to_string_pretty(&data["data"]).unwrap_or_default());
                        SendMessageW(ui.handle(ELEVATION_CANDIDATE),CB_RESETCONTENT,0,0);
                        for candidate in data["data"]["candidates"].as_array().into_iter().flatten(){
                            let label=format!("{} · {} {}",candidate["name"].as_str().unwrap_or_default(),candidate["target"].as_str().unwrap_or_default(),candidate["arguments"].as_str().unwrap_or_default());
                            SendMessageW(ui.handle(ELEVATION_CANDIDATE),CB_ADDSTRING,0,wide(&label).as_ptr() as isize);
                        }
                        SendMessageW(ui.handle(ELEVATION_CANDIDATE),CB_SETCURSEL,usize::MAX,0);
                    }
                    if operation=="bcu-uninstall" { if let Some(items)=data["data"]["junkItems"].as_array(){ui.junk_host=data["computer"].as_str().unwrap_or_default().into();ui.junk_items=items.clone();ui.review_pending=!items.is_empty();} }
                    if operation=="bcu-scan" && data["data"]["applications"].is_array(){
                        ui.records[w as usize].retain(|row|row["id"]!=data["id"]);
                        for app in data["data"]["applications"].as_array().unwrap(){
                            ui.records[w as usize].push(serde_json::json!({
                                "computer":data["computer"],"state":if app["protected"]==true{"受保护"}else{"已扫描"},
                                "operation":app["displayName"],"detail":format!("{} · {}",app["version"].as_str().unwrap_or_default(),app["publisher"].as_str().unwrap_or_default()),"data":app
                            }));
                        }
                    }else if operation=="elevation-list"&&data["data"]["rules"].is_array(){
                        ui.records[w as usize].retain(|row|row["id"]!=data["id"]);
                        for rule in data["data"]["rules"].as_array().unwrap(){
                            ui.records[w as usize].push(serde_json::json!({"computer":data["computer"],"state":"已授权","operation":rule["name"],"detail":rule["target"],"data":rule}));
                        }
                    }else if operation=="connection-repair"{
                        if let Some(row)=ui.records[5].iter_mut().find(|row|row["computer"]==data["computer"]){
                            row["state"]=data["state"].clone();row["detail"]=data["detail"].clone();
                            if row["data"].is_null(){row["data"]=serde_json::json!({});}
                            for field in ["channel","smbReachable","message"]{if !data["data"][field].is_null(){row["data"][field]=data["data"][field].clone();}}
                        }else{ui.records[5].push(data);}
                    }else if operation=="elevation-revoke"{
                        if data["data"]["ruleId"].is_string(){ui.records[10].retain(|row|row["data"]["id"]!=data["data"]["ruleId"]);}
                    }else if matches!(operation,"elevation-create"|"elevation-approve"){
                        if w == 10 {
                            let receipt = model::shortcut_receipt(&data["data"])
                                .unwrap_or_else(|| data["detail"].as_str().unwrap_or_default().to_owned());
                            ui.set(ELEVATION_RECEIPT, &receipt);
                            if ui.page == 10 { ui.layout(); }
                        }
                        if data["data"]["ruleId"].is_string(){
                            let mut data=data;data["data"]["id"]=data["data"]["ruleId"].clone();
                            ui.records[10].retain(|row|row["data"]["id"]!=data["data"]["id"]);ui.records[10].push(data);
                        }
                    }else if matches!(operation,"elevation-preview"|"elevation-discover"){
                    }else if matches!(operation,"bcu-uninstall"|"bcu-delete-junk"){
                        if data["data"]["verifiedRemoved"]==true{
                            ui.records[7].retain(|row|row["data"]["entryXml"]!=data["data"]["selectedEntryXml"]);
                        }
                    }else if !(operation=="sessions"&&matches!(w as usize,7|10)){
                        if let Some(row)=ui.records[w as usize].iter_mut().find(|row|row["id"]==data["id"]){*row=data;}else{ui.records[w as usize].push(data);}
                    }
                    if ui.page as usize==w as usize{ui.refresh_results();}
                    if ui.page==11{ui.load_history();}
                }
                0
            },
            WM_PROGRESS=>{
                let text=*Box::from_raw(l as *mut String);
                if let Ok(data)=serde_json::from_str::<serde_json::Value>(&text){
                    if w==7&&matches!(data["operation"].as_str(),Some("bcu-uninstall"|"bcu-delete-junk")){return 0;}
                    if w==5&&data["operation"]=="connection-repair"{return 0;}
                    if w==10&&data["operation"].as_str().is_some_and(|operation|operation.starts_with("elevation-")){return 0;}
                    if let Some(row)=ui.records[w as usize].iter_mut().find(|row|row["id"]==data["id"]){*row=data;}else{ui.records[w as usize].push(data);}
                    if ui.page as usize==w as usize&&ui.page!=11{ui.refresh_results();}
                    if ui.page==11{ui.load_history();}
                }0
            },
            WM_NOTIFY=>{
                let notification=&*(l as *const NMHDR);
                if notification.idFrom==RESULT_TABLE as usize&&notification.code==LVN_COLUMNCLICK{
                    let click=&*(l as *const NMLISTVIEW);if click.iSubItem>=0{ui.sort_results(click.iSubItem as usize);}return 0;
                }
                if notification.idFrom==DETAIL_TABS as usize&&notification.code==TCN_SELCHANGE{ui.layout();ui.show_inventory_details();}
                if notification.idFrom==RESULT_TABLE as usize&&notification.code==LVN_ITEMCHANGED&&ui.page==3{ui.show_inventory_details();}
                if notification.idFrom==RESULT_TABLE as usize && notification.code==NM_DBLCLK {
                    if ui.page == 7 {
                        let clicked = (*(l as *const NMITEMACTIVATE)).iItem;
                        let selected = SendMessageW(ui.handle(RESULT_TABLE), LVM_GETNEXTITEM, usize::MAX, LVNI_SELECTED as isize);
                        if clicked >= 0 && selected == clicked as isize {
                            ui.launch(Some("bcu-uninstall"));
                        }
                    } else {
                        ui.show_selected_result();
                    }
                    return 0;
                }
                DefWindowProcW(window,msg,w,l)
            },
            WM_DONE=>{
                let text = *Box::from_raw(l as *mut String);
                let elapsed=ui.batches.remove(&(w as usize)).map(|batch|batch.started.elapsed().as_secs_f64()).unwrap_or_default();
                ui.busy=ui.batches.contains_key(&(ui.page as usize));
                if ui.closing {
                    if ui.batches.is_empty(){DestroyWindow(window);}
                    return 0;
                }
                ui.statuses[w as usize] = format!("{text} · 总耗时 {elapsed:.1} 秒");
                if ui.page as usize == w as usize { ui.set(STATUS, &ui.statuses[w as usize]); }
                ui.append_on(w as usize, text);
                for id in [SECONDARY,THIRD,SESSIONS]{EnableWindow(ui.handle(id),1);}
                EnableWindow(ui.handle(CANCEL),0);
                ui.layout();
                if ui.page==7&&ui.review_pending {ui.review_pending=false;ui.review_junk();}
                if ui.page==10&&ui.approve_pending {ui.approve_pending=false;ui.approve_preview();}
                0
            },
            WM_TIMER=>{if w==2{KillTimer(window,2);ui.load_suggestions();return 0;}if ui.page==11&&!ui.closing{ui.load_history();}if ui.busy{SetWindowTextW(window,wide(&format!("企业软件部署 · Rust 复刻核对版 — 执行中 {} 秒",ui.started.elapsed().as_secs())).as_ptr());}else{SetWindowTextW(window,wide(WINDOW_TITLE).as_ptr());}0},
            WM_CLOSE=>{
                for control in [PASSWORD,SOURCE_PASSWORD]{let mut password=ui.text(control);password.zeroize();ui.set(control,"");}
                if !ui.batches.is_empty() {
                    ui.closing = true;
                    for batch in ui.batches.values(){crate::tasks::cancel_batch(&batch.id,&batch.cancel);}
                    KillTimer(window, 1);
                    ShowWindow(window, SW_HIDE);
                } else {
                    DestroyWindow(window);
                }
                0
            },
            WM_DESTROY=>{KillTimer(window,1);PostQuitMessage(0);0},
            _=>DefWindowProcW(window,msg,w,l),
        }
    }
}

pub fn run()->Result<()> {
    crate::tasks::recover_interrupted()?;
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        InitCommonControlsEx(&INITCOMMONCONTROLSEX{dwSize:size_of::<INITCOMMONCONTROLSEX>() as u32,dwICC:ICC_LISTVIEW_CLASSES|ICC_TAB_CLASSES|ICC_TREEVIEW_CLASSES});
        let scale=(GetDpiForSystem() as f64/96.0).min((GetSystemMetrics(SM_CXSCREEN)-80) as f64/1180.0).min((GetSystemMetrics(SM_CYSCREEN)-80) as f64/910.0).max(0.7);
        let instance=GetModuleHandleW(null());let class=wide("CorpDeploymentRustNativeWindow");
        let wc=WNDCLASSW {style:CS_HREDRAW|CS_VREDRAW,lpfnWndProc:Some(procedure),hInstance:instance,hCursor:LoadCursorW(null_mut(),IDC_ARROW),hbrBackground:(COLOR_WINDOW+1) as HBRUSH,lpszClassName:class.as_ptr(),..zeroed()};
        if RegisterClassW(&wc)==0{return Err(win::error("注册原生窗口失败"));}
        let font=CreateFontW((-13.0*scale) as i32,0,0,0,400,0,0,0,DEFAULT_CHARSET as u32,0,0,CLEARTYPE_QUALITY as u32,0,wide("Microsoft YaHei UI").as_ptr());
        let title_font=CreateFontW((-25.0*scale) as i32,0,0,0,600,0,0,0,DEFAULT_CHARSET as u32,0,0,CLEARTYPE_QUALITY as u32,0,wide("Microsoft YaHei UI").as_ptr());
        let mut ui=Box::new(Ui{suggestions:Default::default(),guidance:Default::default(),window:null_mut(),canvas:null_mut(),layout_in_progress:Cell::new(false),controls:Vec::new(),font,title_font,page:0,busy:false,closing:false,cancel:Arc::new(AtomicBool::new(false)),logs:std::array::from_fn(|_| String::new()),statuses:std::array::from_fn(|_| "任务结果 · 就绪".into()),batches:Default::default(),scale,started:Instant::now(),sessions:Vec::new(),inventory:Vec::new(),records:std::array::from_fn(|_|Vec::new()),history:Vec::new(),junk_host:String::new(),junk_items:Vec::new(),junk_selection:Vec::new(),review_pending:false,elevation_preview:None,approve_pending:false,retry_request:None,page_state:Default::default(),_not_send:Cell::new(())});
        let window=CreateWindowExW(0,class.as_ptr(),wide(WINDOW_TITLE).as_ptr(),WS_OVERLAPPEDWINDOW|WS_CLIPCHILDREN,
            CW_USEDEFAULT,CW_USEDEFAULT,(1180.0*scale) as i32,(910.0*scale) as i32,null_mut(),null_mut(),instance,(&mut *ui as *mut Ui).cast());
        if window.is_null(){return Err(win::error("创建原生窗口失败"));}
        ShowWindow(window,SW_SHOW);UpdateWindow(window);
        let mut message=zeroed();
        while GetMessageW(&mut message,null_mut(),0,0)>0 {if IsDialogMessageW(window,&message)==0 {TranslateMessage(&message);DispatchMessageW(&message);}}
        DeleteObject(font);DeleteObject(title_font);Ok(())
    }
}



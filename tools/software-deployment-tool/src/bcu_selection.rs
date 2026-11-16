use crate::{bcu_junk::Item,model::Result,win::{self,wide}};
use std::{mem::zeroed,ptr::{null,null_mut}};
use windows_sys::Win32::{Foundation::*,Graphics::Gdi::*,System::LibraryLoader::*,UI::{Controls::*,Input::KeyboardAndMouse::EnableWindow,WindowsAndMessaging::*}};

struct Selection{items:Vec<Item>,checked:Vec<bool>,visible:Vec<usize>,list:HWND,result:Option<Vec<serde_json::Value>>,done:bool,font:HFONT}
unsafe fn check(list:HWND,index:usize,checked:bool){unsafe{
    let row=LVITEMW{stateMask:LVIS_STATEIMAGEMASK,state:if checked{2<<12}else{1<<12},..zeroed()};
    SendMessageW(list,LVM_SETITEMSTATE,index,(&row as *const LVITEMW) as isize);
}}
unsafe fn save_checks(state:&mut Selection){unsafe{
    for (row,index) in state.visible.iter().enumerate(){state.checked[*index]=state.items[*index].supported&&(SendMessageW(state.list,LVM_GETITEMSTATE,row,LVIS_STATEIMAGEMASK as isize) as u32)>>12==2;}
}}
unsafe fn render(window:HWND,state:&mut Selection){unsafe{
    let filter=SendMessageW(GetDlgItem(window,6),CB_GETCURSEL,0,0);
    let confidence=["","VeryGood","Good","Questionable","Bad","Unknown"].get(filter as usize).copied().unwrap_or_default();
    state.visible=state.items.iter().enumerate().filter(|(_,item)|confidence.is_empty()||item.confidence.eq_ignore_ascii_case(confidence)).map(|(index,_)|index).collect();
    SendMessageW(state.list,WM_SETREDRAW,0,0);SendMessageW(state.list,LVM_DELETEALLITEMS,0,0);
    for (row,index) in state.visible.iter().copied().enumerate(){let item=&state.items[index];
        let texts=[if item.supported{item.confidence.clone()}else{"不支持".into()},item.display_name.clone(),item.kind.clone(),format!("{}{}",item.path,item.value_name.as_ref().map(|name|format!(" · {name}")).unwrap_or_default())];
        for (column,text) in texts.iter().enumerate(){let mut text=wide(text);let value=LVITEMW{mask:LVIF_TEXT,iItem:row as i32,iSubItem:column as i32,pszText:text.as_mut_ptr(),..zeroed()};SendMessageW(state.list,if column==0{LVM_INSERTITEMW}else{LVM_SETITEMW},0,(&value as *const LVITEMW) as isize);}
        check(state.list,row,state.checked[index]);
    }
    SendMessageW(state.list,WM_SETREDRAW,1,0);InvalidateRect(state.list,null(),1);
}}
unsafe extern "system" fn procedure(window:HWND,message:u32,w:WPARAM,l:LPARAM)->LRESULT{unsafe{
    if message==WM_NCCREATE{
        let create=&*(l as *const CREATESTRUCTW);SetWindowLongPtrW(window,GWLP_USERDATA,create.lpCreateParams as isize);
    }
    let pointer=GetWindowLongPtrW(window,GWLP_USERDATA) as *mut Selection;
    if pointer.is_null(){return DefWindowProcW(window,message,w,l);}
    let state=&mut *pointer;
    match message{
        WM_COMMAND=>{
            match (w&0xffff) as i32{
                1=>{
                    save_checks(state);
                    let mut selected=Vec::new();
                    for (index,item) in state.items.iter().enumerate(){
                        if state.checked[index]{
                            if !item.supported{win::message("不支持的残留项","请取消勾选不支持删除的残留项。");return 0;}
                            if let Err(error)=item.validate(){win::message("残留项校验失败",&error);return 0;}
                            selected.push(serde_json::to_value(item).unwrap());
                        }
                    }
                    if selected.is_empty(){state.result=Some(selected);DestroyWindow(window);return 0;}
                    state.result=Some(selected);DestroyWindow(window);
                },
                2=>{DestroyWindow(window);},
                3|4|5=>{for (index,item) in state.items.iter().enumerate(){state.checked[index]=item.supported&&match (w&0xffff) as i32{3=>item.default_selected(),4=>true,_=>false};}render(window,state);},
                6 if (w>>16) as u32==CBN_SELCHANGE=>{save_checks(state);render(window,state);},
                _=>()
            }
            0
        },
        WM_SIZE=>{
            let width=(l&0xffff) as i32;let height=((l>>16)&0xffff) as i32;
            MoveWindow(state.list,16,100,(width-32).max(1),(height-166).max(1),1);
            MoveWindow(GetDlgItem(window,6),16,58,250,220,1);
            for (id,x) in [(3,16),(4,170),(5,284),(2,width-252),(1,width-138)]{
                MoveWindow(GetDlgItem(window,id),x,height-48,if id==3{140}else{102},32,1);
            }
            0
        },
        WM_CLOSE=>{DestroyWindow(window);0},
        WM_GETMINMAXINFO=>{let info=&mut *(l as *mut MINMAXINFO);info.ptMinTrackSize=POINT{x:780,y:400};0},
        WM_DESTROY=>{state.done=true;0},
        WM_NCDESTROY=>{SetWindowLongPtrW(window,GWLP_USERDATA,0);DefWindowProcW(window,message,w,l)},
        _=>DefWindowProcW(window,message,w,l)
    }
}}
pub fn select(owner:HWND,computer:&str,values:&[serde_json::Value])->Result<Option<Vec<serde_json::Value>>>{
    let mut items=values.iter().cloned().map(serde_json::from_value::<Item>).collect::<std::result::Result<Vec<_>,_>>().map_err(|e|e.to_string())?;
    if items.len()>500{return Err("残留项超过 500 个，请分批处理。".into());}
    let rank=|item:&Item|match item.confidence.as_str(){"VeryGood"=>0,"Good"=>1,"Questionable"=>2,"Bad"=>3,_=>4};
    items.sort_by_key(|item|(rank(item),item.display_name.to_lowercase()));
    unsafe{
        let instance=GetModuleHandleW(null());let class=wide("CorpRustBcuJunkSelection");
        let definition=WNDCLASSW{lpfnWndProc:Some(procedure),hInstance:instance,hCursor:LoadCursorW(null_mut(),IDC_ARROW),hbrBackground:(COLOR_WINDOW+1) as HBRUSH,lpszClassName:class.as_ptr(),..zeroed()};
        RegisterClassW(&definition);
        let font=CreateFontW(-16,0,0,0,400,0,0,0,DEFAULT_CHARSET as u32,0,0,DEFAULT_QUALITY as u32,0,wide("Microsoft YaHei UI").as_ptr());
        let mut state=Box::new(Selection{checked:items.iter().map(Item::default_selected).collect(),visible:Vec::new(),items,list:null_mut(),result:None,done:false,font});
        let mut bounds:RECT=zeroed();GetWindowRect(owner,&mut bounds);
        let width=940.min(GetSystemMetrics(SM_CXSCREEN)-40);let height=640.min(GetSystemMetrics(SM_CYSCREEN)-60);
        let window=CreateWindowExW(WS_EX_DLGMODALFRAME,class.as_ptr(),wide(&format!("卸载残留 — {computer}")).as_ptr(),WS_OVERLAPPED|WS_CAPTION|WS_SYSMENU|WS_THICKFRAME,
            (bounds.left+30).max(0),(bounds.top+30).max(0),width,height,owner,null_mut(),instance,(&mut *state as *mut Selection).cast());
        if window.is_null(){DeleteObject(font);return Err(win::error("创建残留选择窗口失败"));}
        let label=CreateWindowExW(0,wide("STATIC").as_ptr(),wide("仅默认勾选可信度 VeryGood 且支持清理的项目。可更改选择，或保留全部残留。\n选择清理前请核对完整路径，较低可信度的项目可能与其他软件共用。").as_ptr(),WS_CHILD|WS_VISIBLE,16,12,width-48,48,window,null_mut(),instance,null());
        SendMessageW(label,WM_SETFONT,font as usize,1);
        state.list=CreateWindowExW(WS_EX_CLIENTEDGE,wide("SysListView32").as_ptr(),null(),WS_CHILD|WS_VISIBLE|WS_TABSTOP|LVS_REPORT|LVS_SHOWSELALWAYS,16,66,width-48,height-170,window,100usize as HMENU,instance,null());
        SendMessageW(state.list,WM_SETFONT,font as usize,1);
        windows_sys::Win32::UI::Shell::SetWindowSubclass(state.list,Some(win::listview_scroll),2,0);
        SendMessageW(state.list,LVM_SETEXTENDEDLISTVIEWSTYLE,0,(LVS_EX_CHECKBOXES|LVS_EX_FULLROWSELECT|LVS_EX_DOUBLEBUFFER) as isize);
        for (index,(text,width)) in [("可信度",105),("残留项目",185),("类型",110),("完整路径 / 值名",440)].iter().enumerate(){
            let mut text=wide(text);let column=LVCOLUMNW{mask:LVCF_TEXT|LVCF_WIDTH,cx:*width,pszText:text.as_mut_ptr(),..zeroed()};
            SendMessageW(state.list,LVM_INSERTCOLUMNW,index,(&column as *const LVCOLUMNW) as isize);
        }
        let filter=CreateWindowExW(0,wide("COMBOBOX").as_ptr(),null(),WS_CHILD|WS_VISIBLE|WS_TABSTOP|WS_VSCROLL|CBS_DROPDOWNLIST as u32,16,58,250,220,window,6usize as HMENU,instance,null());
        SendMessageW(filter,WM_SETFONT,font as usize,1);
        for item in ["全部等级","VeryGood","Good","Questionable","Bad","Unknown"]{SendMessageW(filter,CB_ADDSTRING,0,wide(item).as_ptr() as isize);}
        SendMessageW(filter,CB_SETCURSEL,0,0);render(window,&mut state);
        for (id,text) in [(3,"仅勾选 VeryGood"),(4,"全选可删除项"),(5,"全不选"),(2,"保留并关闭"),(1,"删除勾选项")]{
            let handle=CreateWindowExW(0,wide("BUTTON").as_ptr(),wide(text).as_ptr(),WS_CHILD|WS_VISIBLE|WS_TABSTOP|BS_PUSHBUTTON as u32,0,0,1,1,window,id as usize as HMENU,instance,null());
            SendMessageW(handle,WM_SETFONT,font as usize,1);
        }
        let mut client:RECT=zeroed();GetClientRect(window,&mut client);
        SendMessageW(window,WM_SIZE,0,((client.bottom as usize)<<16|client.right as usize) as isize);
        EnableWindow(owner,0);ShowWindow(window,SW_SHOW);SetForegroundWindow(window);
        let mut message:MSG=zeroed();let mut quit=None;
        while !state.done{
            let code=GetMessageW(&mut message,null_mut(),0,0);
            if code<=0{if code==0{quit=Some(message.wParam);}if IsWindow(window)!=0{DestroyWindow(window);}break;}
            if IsDialogMessageW(window,&message)==0{TranslateMessage(&message);DispatchMessageW(&message);}
        }
        EnableWindow(owner,1);SetForegroundWindow(owner);DeleteObject(state.font);
        if let Some(code)=quit{PostQuitMessage(code as i32);}
        Ok(state.result.take())
    }
}

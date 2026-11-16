use super::*;
pub(super) const SCROLL_MESSAGE:u32=WM_APP+20;
pub(super) const WHEEL_MESSAGE:u32=WM_APP+21;
unsafe extern "system" fn procedure(window:HWND,message:u32,w:WPARAM,l:LPARAM)->LRESULT{unsafe{
    let parent=GetParent(window);
    match message{
        WM_COMMAND|WM_NOTIFY|WM_DRAWITEM|WM_MEASUREITEM|WM_CTLCOLORSTATIC|WM_CTLCOLORBTN|WM_CTLCOLOREDIT=>SendMessageW(parent,message,w,l),
        WM_VSCROLL=>SendMessageW(parent,SCROLL_MESSAGE,w,l),
        WM_MOUSEWHEEL=>SendMessageW(parent,WHEEL_MESSAGE,w,l),
        WM_ERASEBKGND=>1,
        WM_PAINT=>{
            let mut state:PAINTSTRUCT=zeroed();let dc=BeginPaint(window,&mut state);
            let pointer=GetWindowLongPtrW(parent,GWLP_USERDATA) as *const Ui;
            if !pointer.is_null(){paint(&*pointer,dc);}EndPaint(window,&state);0
        },
        _=>DefWindowProcW(window,message,w,l)
    }
}}
pub(super) unsafe fn attach(ui:&mut Ui){unsafe{
    let class=wide("CorpRustPageCanvas");let instance=GetModuleHandleW(null());
    let wc=WNDCLASSW{lpfnWndProc:Some(procedure),hInstance:instance,hCursor:LoadCursorW(null_mut(),IDC_ARROW),lpszClassName:class.as_ptr(),..zeroed()};RegisterClassW(&wc);
    ui.canvas=CreateWindowExW(WS_EX_CONTROLPARENT,class.as_ptr(),null(),WS_CHILD|WS_VISIBLE|WS_VSCROLL|WS_CLIPCHILDREN,0,0,1,1,ui.window,null_mut(),instance,null());
    for control in &ui.controls{if (control.x>=236||control.id==528)&&!matches!(control.id,TITLE|529|CLEAR_TARGETS){SetParent(control.handle,ui.canvas);}}
}}
pub(super) fn minimum(ui:&Ui)->i32{match ui.page{0=>1430,1=>860,3=>880,4=>650,5=>730,7=>850,8=>760,9=>930,10=>930+ui.elevation_receipt_height()+if ui.checked(ELEVATION_ADVANCED){420}else{0}+if ui.checked(ELEVATION_MANAGE){240}else{0},_=>700}}
pub(super) unsafe fn layout(ui:&Ui,rect:&mut RECT){unsafe{
    let px=|n:i32|(n as f64*ui.scale).round() as i32;let real_bottom=rect.bottom;
    let virtual_bottom=real_bottom.max(px(minimum(ui)));
    SetWindowPos(ui.canvas,null_mut(),px(205),px(88),(rect.right-px(205)).max(1),(real_bottom-px(88)).max(1),SWP_NOZORDER|SWP_NOACTIVATE|SWP_NOREDRAW|SWP_NOCOPYBITS);
    let maximum=(virtual_bottom-real_bottom).max(0);let scroll=ui.page_state.scroll[ui.page as usize].get().clamp(0,maximum);
    ui.page_state.scroll[ui.page as usize].set(scroll);
    SetScrollInfo(ui.canvas,SB_VERT,&SCROLLINFO{cbSize:size_of::<SCROLLINFO>() as u32,fMask:SIF_RANGE|SIF_PAGE|SIF_POS,nMin:0,nMax:virtual_bottom-px(88)-1,nPage:(real_bottom-px(88)).max(1) as u32,nPos:scroll,..zeroed()},1);
    rect.bottom=virtual_bottom;if maximum>0{rect.right-=GetSystemMetrics(SM_CXVSCROLL);}
}}
pub(super) unsafe fn move_control(ui:&Ui,control:HWND,mut x:i32,mut y:i32,width:i32,height:i32){unsafe{
    if !ui.canvas.is_null()&&GetParent(control)==ui.canvas{
        x-=(205.0*ui.scale).round() as i32;y-=(88.0*ui.scale).round() as i32+ui.page_state.scroll[ui.page as usize].get();
    }
    SetWindowPos(control,null_mut(),x,y,width,height,SWP_NOZORDER|SWP_NOACTIVATE|SWP_NOREDRAW|SWP_NOCOPYBITS);
}}
pub(super) fn scroll(ui:&Ui,w:WPARAM,wheel:bool){unsafe{
    let mut info:SCROLLINFO=zeroed();info.cbSize=size_of::<SCROLLINFO>() as u32;info.fMask=SIF_ALL;GetScrollInfo(ui.canvas,SB_VERT,&mut info);
    let step=(48.0*ui.scale).round() as i32;let current=ui.page_state.scroll[ui.page as usize].get();
    let next=if wheel{current-(((w>>16) as u16 as i16) as i32/120)*step}else{match (w&0xffff) as i32{
        SB_LINEUP=>current-step,SB_LINEDOWN=>current+step,SB_PAGEUP=>current-info.nPage as i32+step,SB_PAGEDOWN=>current+info.nPage as i32-step,
        SB_THUMBPOSITION|SB_THUMBTRACK=>info.nTrackPos,SB_TOP=>0,SB_BOTTOM=>info.nMax,_=>current
    }};
    let next=next.clamp(0,(info.nMax-info.nPage as i32+1).max(0));if next==current{return;}
    ui.page_state.scroll[ui.page as usize].set(next);
    let visible=ui.controls.iter().filter(|control|GetParent(control.handle)==ui.canvas&&IsWindowVisible(control.handle)!=0).collect::<Vec<_>>();
    let mut batch=BeginDeferWindowPos(visible.len() as i32);
    if batch.is_null(){ui.layout();return;}
    for control in visible{
        let mut rect:RECT=zeroed();GetWindowRect(control.handle,&mut rect);
        MapWindowPoints(null_mut(),ui.canvas,(&mut rect as *mut RECT).cast(),2);
        batch=DeferWindowPos(batch,control.handle,null_mut(),rect.left,rect.top+current-next,0,0,SWP_NOSIZE|SWP_NOZORDER|SWP_NOACTIVATE|SWP_NOREDRAW|SWP_NOCOPYBITS);
        if batch.is_null(){ui.layout();return;}
    }
    if EndDeferWindowPos(batch)==0{ui.layout();return;}
    SetScrollInfo(ui.canvas,SB_VERT,&SCROLLINFO{cbSize:size_of::<SCROLLINFO>() as u32,fMask:SIF_POS,nPos:next,..zeroed()},1);
    RedrawWindow(ui.canvas,null(),null_mut(),RDW_INVALIDATE|RDW_ERASE|RDW_ALLCHILDREN|RDW_UPDATENOW);
}}
pub(super) fn background(page:i32)->u32{if matches!(page,0|1|3|8|9|11){0x00f5f1ed}else{0x00f8f7f7}}
unsafe fn paint(ui:&Ui,dc:HDC){unsafe{
    let mut rect:RECT=zeroed();GetClientRect(ui.canvas,&mut rect);let bg=CreateSolidBrush(background(ui.page));FillRect(dc,&rect,bg);DeleteObject(bg);
    let px=|n:i32|(n as f64*ui.scale).round() as i32;let offset=ui.page_state.scroll[ui.page as usize].get();
    let width=(rect.right as f64/ui.scale) as i32-36;let bottom=minimum(ui).max(((rect.bottom+px(88)) as f64/ui.scale) as i32);
    let card=|x:i32,y:i32,w:i32,h:i32,color:u32|{
        let brush=CreateSolidBrush(color);let pen=CreatePen(PS_SOLID,1,0x00ece8e6);let old_brush=SelectObject(dc,brush);let old_pen=SelectObject(dc,pen);
        RoundRect(dc,px(x),px(y-88)-offset,px(x+w),px(y+h-88)-offset,px(8),px(8));SelectObject(dc,old_brush);SelectObject(dc,old_pen);DeleteObject(brush);DeleteObject(pen);
    };
    match ui.page{
        0=>{card(18,100,width,72,0xffffff);card(18,180,width-324,900,0xffffff);card(width-292,180,310,590,0xffffff);card(18,1090,width,bottom-1160,0xffffff);},
        1=>{card(18,100,width,480,0xffffff);let half=(width-18)/2;card(24,180,half-12,228,0x00fcfaf8);card(half+42,180,half-12,228,0x00fcfaf8);card(18,584,width,bottom-650,0xffffff);},
        3=>{card(18,100,width,224,0xffffff);card(18,332,width,bottom-396,0xffffff);},
        5=>{card(18,100,width,146,0xffffff);card(18,254,width,bottom-320,0xffffff);},
        7=>{card(18,100,width,194,0xffffff);card(18,298,width,bottom-364,0xffffff);},
        8=>{card(18,100,width,292,0xffffff);card(18,400,width,bottom-466,0xffffff);},
        9=>{card(18,100,width,326,0xffffff);card(18,438,width,bottom-496,0xffffff);},
        10=>card(32,194,width-28,534,0xffffff),
        11=>{card(18,100,width,122,0x002d2118);card(18,232,width,bottom-298,0xffffff);},_=>()
    }
}}

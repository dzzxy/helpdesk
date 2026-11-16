use super::*;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetCapture, ReleaseCapture, SetCapture, SetFocus};

pub(super) const CONTROL: i32 = 752;
pub(super) const MOVE: u32 = WM_APP + 27;
pub(super) const CLASS: &str = "CorpRustVerticalSplitter";
const HEIGHT: i32 = 12;

#[derive(Clone, Copy)]
pub(super) struct Panes {
    pub page: i32,
    pub left: i32,
    pub width: i32,
    pub bottom: i32,
}

impl Panes {
    fn limits(self) -> (i32, i32, i32) {
        match self.page {
            3 => (374, 374 + (self.bottom - 420) / 2, self.bottom - 70 - 122),
            7 => (390, self.bottom - 215, self.bottom - 76 - 100),
            9 => (480, self.bottom - 230, self.bottom - 66 - 100),
            _ => unreachable!(),
        }
    }

    fn position(self, offset: i32) -> i32 {
        let (top, default, maximum) = self.limits();
        let minimum = top + 100;
        (default + offset).clamp(minimum, maximum.max(minimum))
    }

    pub fn apply(self, ui: &Ui) {
        let page = self.page as usize;
        let (top, default, _) = self.limits();
        let split = self.position(ui.page_state.split_offset[page].get());
        ui.page_state.split_offset[page].set(split - default);
        let lower = split + HEIGHT;
        let px = |n: i32| (n as f64 * ui.scale).round() as i32;
        let place = |id, x, y, w, h| unsafe {
            let control = ui.handle(id);
            canvas::move_control(ui, control, px(x), px(y), px(w), px(h));
            if IsWindowVisible(control) == 0 { ShowWindow(control, SW_SHOWNA); }
        };
        let x = self.left;
        let w = self.width;
        place(RESULT_TABLE, x, top, w, split - top);
        place(CONTROL, x, split, w, HEIGHT);
        match self.page {
            3 => {
                let tab = unsafe { SendMessageW(ui.handle(DETAIL_TABS), TCM_GETCURSEL, 0, 0) };
                place(DETAIL_TABS, x, lower, w, 32);
                place(if tab == 0 { DETAIL_OVERVIEW } else { DETAIL_TABLE },
                    x, lower + 34, w, self.bottom - 70 - lower - 34);
            }
            7 => place(LOG, x, lower, w, self.bottom - 76 - lower),
            9 => {
                place(LOG, x, lower, w - 226, self.bottom - 66 - lower);
                place(OPEN_BUNDLE, x + w - 208, lower + 4, 208, 38);
                place(OPEN_BUNDLE_FOLDER, x + w - 208, lower + 52, 208, 34);
            }
            _ => unreachable!(),
        }
    }
}

pub(super) fn layout(ui: &Ui, left: i32, width: i32, bottom: i32) {
    let panes = Panes { page: ui.page, left, width, bottom };
    ui.page_state.panes.set(Some(panes));
    panes.apply(ui);
}

pub(super) fn adjust(ui: &Ui, pixels: i32, reset: bool) {
    let Some(panes) = ui.page_state.panes.get().filter(|panes| panes.page == ui.page) else { return; };
    let offset = &ui.page_state.split_offset[ui.page as usize];
    let delta = (pixels as f64 / ui.scale).round() as i32;
    offset.set(if reset { 0 } else { offset.get().saturating_add(delta) });
    panes.apply(ui);
    unsafe { RedrawWindow(ui.canvas, null(), null_mut(), RDW_INVALIDATE | RDW_ALLCHILDREN); }
}

pub(super) unsafe fn register() { unsafe {
    let class = wide(CLASS);
    let definition = WNDCLASSW {
        style: CS_DBLCLKS,
        lpfnWndProc: Some(procedure),
        hInstance: GetModuleHandleW(null()),
        hCursor: LoadCursorW(null_mut(), IDC_SIZENS),
        lpszClassName: class.as_ptr(),
        ..zeroed()
    };
    RegisterClassW(&definition);
}}

unsafe extern "system" fn procedure(window: HWND, message: u32, w: WPARAM, l: LPARAM) -> LRESULT { unsafe {
    let manager = GetParent(GetParent(window));
    match message {
        WM_LBUTTONDOWN => {
            let mut point = POINT::default();
            if GetCursorPos(&mut point) != 0 {
                SetWindowLongPtrW(window, GWLP_USERDATA, point.y as isize);
                SetFocus(window);
                SetCapture(window);
            }
            0
        }
        WM_MOUSEMOVE if GetCapture() == window => {
            let mut point = POINT::default();
            if GetCursorPos(&mut point) != 0 {
                let previous = GetWindowLongPtrW(window, GWLP_USERDATA) as i32;
                if point.y != previous {
                    SetWindowLongPtrW(window, GWLP_USERDATA, point.y as isize);
                    SendMessageW(manager, MOVE, 0, (point.y - previous) as isize);
                }
            }
            0
        }
        WM_LBUTTONUP if GetCapture() == window => { ReleaseCapture(); 0 }
        WM_CANCELMODE => { if GetCapture() == window { ReleaseCapture(); } 0 }
        WM_LBUTTONDBLCLK => { SendMessageW(manager, MOVE, 1, 0); 0 }
        WM_GETDLGCODE => DLGC_WANTARROWS as isize,
        WM_KEYDOWN if matches!(w, 0x26 | 0x28 | 0x24) => {
            SendMessageW(manager, MOVE, usize::from(w == 0x24), if w == 0x26 { -12 } else { 12 });
            0
        }
        WM_SETFOCUS | WM_KILLFOCUS => { InvalidateRect(window, null(), 0); 0 }
        WM_PAINT => {
            let mut paint: PAINTSTRUCT = zeroed();
            let dc = BeginPaint(window, &mut paint);
            let mut rect: RECT = zeroed();
            GetClientRect(window, &mut rect);
            FillRect(dc, &rect, GetStockObject(WHITE_BRUSH));
            let y = rect.bottom / 2;
            let line = RECT { left: (rect.right - 54) / 2, top: y - 2, right: (rect.right + 54) / 2, bottom: y + 2 };
            SetDCBrushColor(dc, 0x00a79a90);
            FillRect(dc, &line, GetStockObject(DC_BRUSH));
            if windows_sys::Win32::UI::Input::KeyboardAndMouse::GetFocus() == window { DrawFocusRect(dc, &rect); }
            EndPaint(window, &paint);
            0
        }
        _ => DefWindowProcW(window, message, w, l),
    }
}}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dragging_keeps_both_panes_visible_at_minimum_page_height() {
        for (page, bottom, footer, lower_min) in [(3, 880, 70, 110), (7, 850, 76, 88), (9, 930, 66, 88)] {
            let panes = Panes { page, left: 241, width: 720, bottom };
            let (top, _, _) = panes.limits();
            assert_eq!(panes.position(-10_000) - top, 100);
            assert_eq!(bottom - footer - panes.position(10_000) - HEIGHT, lower_min);
            assert!(panes.position(20) > panes.position(0));
        }
    }
}

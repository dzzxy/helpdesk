#![windows_subsystem = "windows"]

use base64::{engine::general_purpose::STANDARD, Engine as _};
use std::fmt::Write as _;
use std::io::Write as _;
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::ptr::{null, null_mut};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::{GetStockObject, COLOR_WINDOW, DEFAULT_GUI_FONT};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::EnableWindow;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

const SCRIPT: &str = include_str!("../bios.ps1");
const BOOTSTRAP: &str = "[Console]::InputEncoding=[Text.UTF8Encoding]::new($false); & ([scriptblock]::Create([Text.Encoding]::UTF8.GetString([Convert]::FromBase64String([Console]::ReadLine()))))";
const TITLE: &str = "DELL 主板网络唤醒";
const REFRESH: usize = 101;
const SAVE: usize = 102;

const BIOS_TIMER: usize = 1;

struct Setting {
    name: String,
    current: String,
    target: String,
    choices: Vec<String>,
    writable: bool,
}

struct Preview {
    title: String,
    token: String,
    settings: Vec<Setting>,
    status: String,
    needs_password: bool,
}

impl Preview {
    fn display(&self) -> String {
        let mut text = String::new();
        for setting in &self.settings {
            let current = if setting.current == "Unavailable" {
                "固件未提供"
            } else {
                &setting.current
            };
            writeln!(
                text,
                "{}\n    当前 {}，建议 {}，可选 {}\n",
                setting.name,
                current,
                setting.target,
                setting.choices.join(" / ")
            )
            .expect("writing to a string");
        }
        text
    }
}

fn run_backend(
    operation: &str,
    argument: &str,
    targets: &[String],
    password: &str,
) -> Result<String, String> {
    let encoded = STANDARD.encode(
        BOOTSTRAP
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>(),
    );
    let system = std::env::var_os("SystemRoot").ok_or("无法定位 Windows PowerShell。")?;
    let exe = PathBuf::from(system).join(r"System32\WindowsPowerShell\v1.0\powershell.exe");
    let mut child = Command::new(exe)
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-EncodedCommand",
        ])
        .arg(encoded)
        .creation_flags(0x08000000)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("启动 PowerShell 失败（{error}）"))?;
    let mut input = child.stdin.take().expect("piped stdin");
    let sent = (|| {
        writeln!(input, "{}", STANDARD.encode(SCRIPT.as_bytes()))?;
        writeln!(input, "{operation}\n{argument}")?;
        writeln!(input, "{password}")?;
        for index in 0..2 {
            writeln!(input, "{}", targets.get(index).map_or("", String::as_str))?;
        }
        Ok::<_, std::io::Error>(())
    })();
    drop(input);
    let output = child
        .wait_with_output()
        .map_err(|error| format!("读取 BIOS 工具结果失败（{error}）"))?;
    sent.map_err(|error| format!("提交 BIOS 工具请求失败（{error}）"))?;
    let reply = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if let Some(reason) = reply.strip_prefix("ERROR|") {
        return Err(reason.into());
    }
    if !output.status.success() {
        return Err("后台操作失败。".into());
    }
    Ok(reply)
}

fn run_bios(
    operation: &str,
    token: &str,
    targets: &[String],
    password: &str,
) -> Result<Preview, String> {
    let reply = run_backend(operation, token, targets, password)?;
    let mut preview = Preview {
        title: String::new(),
        token: String::new(),
        settings: Vec::new(),
        status: String::new(),
        needs_password: false,
    };
    for line in reply.lines() {
        let fields: Vec<_> = line.split('|').collect();
        match fields.as_slice() {
            ["BOARD", product, bios] => {
                preview.title = format!("{product} 网络唤醒（BIOS {bios}）")
            }
            ["TOKEN", value] => preview.token = (*value).into(),
            ["AUTH", "Manual"] => preview.needs_password = true,
            ["AUTH", "None" | "Unknown"] => {}
            ["ROW", name, current, target, choices, writable] => preview.settings.push(Setting {
                writable: *writable == "1",
                name: (*name).into(),
                current: (*current).into(),
                target: (*target).into(),
                choices: choices
                    .split(',')
                    .filter(|value| !value.is_empty())
                    .map(str::to_owned)
                    .collect(),
            }),
            ["STATUS", encoded] => {
                preview.status =
                    String::from_utf8(STANDARD.decode(encoded).map_err(|_| "状态编码无效。")?)
                        .map_err(|_| "状态文本无效。")?;
            }
            _ => return Err("收到了无法识别的 BIOS 工具输出。".into()),
        }
    }
    if preview.title.is_empty() || preview.token.len() != 64 || preview.settings.len() != 2 {
        return Err("未收到完整的 BIOS 预览结果。".into());
    }
    Ok(preview)
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain([0]).collect()
}

struct Editor {
    labels: [HWND; 2],
    current_values: [HWND; 2],
    values: [HWND; 2],
    recommendations: [HWND; 2],
    save: HWND,
    refresh: HWND,
    status: HWND,
    password_label: HWND,
    password: HWND,
    preview: Option<Preview>,
    pending: Option<Pending>,
}

struct Pending {
    result: Receiver<Result<Preview, String>>,
    saving: bool,
}

enum Request {
    Read,
    Apply(String, Vec<String>, String),
}

impl Editor {
    unsafe fn start(&mut self, hwnd: HWND, request: Request) {
        if self.pending.is_some() {
            return;
        }
        let saving = matches!(request, Request::Apply(..));
        let status = match &request {
            Request::Read => "正在读取 BIOS 设置…",
            Request::Apply(..) => "正在写入并回读 BIOS 设置…",
        };
        if SetTimer(hwnd, BIOS_TIMER, 100, None) == 0 {
            SetWindowTextW(self.status, wide("无法启动操作计时器，请重试。").as_ptr());
            return;
        }
        let (sender, result) = mpsc::channel();
        let worker = std::thread::Builder::new().spawn(move || {
            let outcome = match request {
                Request::Apply(token, targets, password) => {
                    run_bios("apply", &token, &targets, &password)
                        .map_err(|error| format!("写入结果未获确认。请刷新检查当前值。\n{error}"))
                }
                Request::Read => run_bios("read", "", &[], ""),
            };
            let _ = sender.send(outcome);
        });
        if let Err(error) = worker {
            KillTimer(hwnd, BIOS_TIMER);
            SetWindowTextW(
                self.status,
                wide(&format!("无法启动后台操作（{error}）。")).as_ptr(),
            );
            return;
        }
        self.pending = Some(Pending { result, saving });
        for &control in &self.values {
            EnableWindow(control, 0);
        }
        EnableWindow(self.save, 0);
        EnableWindow(self.refresh, 0);
        EnableWindow(self.password, 0);
        SetWindowTextW(self.status, wide(status).as_ptr());
    }

    unsafe fn poll(&mut self, hwnd: HWND) {
        let Some(pending) = &self.pending else { return };
        let result = match pending.result.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err("后台操作已中断。请刷新检查当前值。".into()),
        };
        let saving = pending.saving;
        self.pending = None;
        KillTimer(hwnd, BIOS_TIMER);
        self.load(hwnd, result, saving);
        EnableWindow(self.refresh, 1);
        EnableWindow(self.password, 1);
    }

    unsafe fn load(&mut self, hwnd: HWND, result: Result<Preview, String>, _saved: bool) {
        let mut can_edit = false;
        ShowWindow(self.password_label, SW_HIDE);
        ShowWindow(self.password, SW_HIDE);
        SetWindowTextW(self.password, wide("").as_ptr());
        let status = match result {
            Ok(preview) => {
                SetWindowTextW(hwnd, wide(&preview.title).as_ptr());
                for (index, setting) in preview.settings.iter().enumerate() {
                    SetWindowTextW(self.labels[index], wide(&setting.name).as_ptr());
                    let current = if setting.choices.is_empty() {
                        "固件未提供"
                    } else {
                        &setting.current
                    };
                    SetWindowTextW(self.current_values[index], wide(current).as_ptr());
                    SetWindowTextW(self.recommendations[index], wide(&setting.target).as_ptr());
                    let combo = self.values[index];
                    SendMessageW(combo, CB_RESETCONTENT, 0, 0);
                    let choices = &setting.choices;
                    if choices.is_empty() {
                        SendMessageW(combo, CB_ADDSTRING, 0, wide("固件未提供").as_ptr() as isize);
                        SendMessageW(combo, CB_SETCURSEL, 0, 0);
                    } else {
                        for choice in choices {
                            SendMessageW(combo, CB_ADDSTRING, 0, wide(choice).as_ptr() as isize);
                        }
                        if let Some(current) =
                            choices.iter().position(|choice| choice == &setting.current)
                        {
                            SendMessageW(combo, CB_SETCURSEL, current, 0);
                        }
                    }
                    let editable = setting.writable && choices.len() > 1;
                    EnableWindow(combo, editable.into());
                    can_edit |= editable;
                }
                let message = preview.status.clone();
                if can_edit && preview.needs_password {
                    ShowWindow(self.password_label, SW_SHOW);
                    ShowWindow(self.password, SW_SHOW);
                }
                self.preview = Some(preview);
                message
            }
            Err(error) => {
                self.preview = None;
                for &combo in &self.values {
                    EnableWindow(combo, 0);
                }
                error
            }
        };
        EnableWindow(self.save, can_edit.into());
        SetWindowTextW(self.status, wide(&status).as_ptr());
    }

    unsafe fn save(&mut self, hwnd: HWND) {
        if self.pending.is_some() {
            return;
        }
        let Some(preview) = &self.preview else { return };
        let mut targets = Vec::with_capacity(2);
        let mut changes = String::new();
        for (index, setting) in preview.settings.iter().enumerate() {
            let selected = SendMessageW(self.values[index], CB_GETCURSEL, 0, 0) as usize;
            let value = if setting.writable {
                setting.choices.get(selected).cloned().unwrap_or_default()
            } else {
                String::new()
            };
            if !value.is_empty() && value != setting.current {
                let _ = writeln!(
                    changes,
                    "{}\n    {} → {}",
                    setting.name, setting.current, value
                );
            }
            targets.push(value);
        }
        if changes.is_empty() {
            SetWindowTextW(self.status, wide("当前没有需要写入的变更。").as_ptr());
            return;
        }
        let message = format!("{changes}\n是否保存这些 BIOS 变更？保存后请自行重启。");
        let answer = MessageBoxW(
            hwnd,
            wide(&message).as_ptr(),
            wide(&preview.title).as_ptr(),
            MB_YESNO | MB_ICONQUESTION | MB_DEFBUTTON2,
        );
        if answer == IDYES {
            let password = if preview.needs_password {
                let mut buffer = vec![0; GetWindowTextLengthW(self.password) as usize + 1];
                let size = GetWindowTextW(self.password, buffer.as_mut_ptr(), buffer.len() as i32);
                let password = String::from_utf16_lossy(&buffer[..size as usize]);
                buffer.fill(0);
                if password.is_empty() {
                    SetWindowTextW(self.status, wide("请输入 BIOS 管理员密码。").as_ptr());
                    return;
                }
                password
            } else {
                String::new()
            };
            SetWindowTextW(self.password, wide("").as_ptr());
            self.start(
                hwnd,
                Request::Apply(preview.token.clone(), targets, password),
            );
        }
    }
}

unsafe fn control(
    parent: HWND,
    class: &str,
    text: &str,
    style: u32,
    id: usize,
    bounds: [i32; 4],
) -> HWND {
    let [x, y, width, height] = bounds;
    let window = CreateWindowExW(
        0,
        wide(class).as_ptr(),
        wide(text).as_ptr(),
        WS_CHILD | WS_VISIBLE | style,
        x,
        y,
        width,
        height,
        parent,
        id as HMENU,
        GetModuleHandleW(null()),
        null(),
    );
    SendMessageW(
        window,
        WM_SETFONT,
        GetStockObject(DEFAULT_GUI_FONT) as usize,
        1,
    );
    window
}

unsafe fn password_control(
    parent: HWND,
    class: &str,
    text: &str,
    style: u32,
    bounds: [i32; 4],
) -> HWND {
    let handle = control(parent, class, text, style, 0, bounds);
    ShowWindow(handle, SW_HIDE);
    handle
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_CREATE => {
            control(hwnd, "STATIC", "BIOS 设置", 0, 0, [16, 16, 270, 22]);
            control(hwnd, "STATIC", "当前值", 0, 0, [302, 16, 105, 22]);
            control(hwnd, "STATIC", "设置为", 0, 0, [410, 16, 175, 22]);
            control(hwnd, "STATIC", "WoL 推荐值", 0, 0, [600, 16, 105, 22]);
            let editor = Editor {
                labels: std::array::from_fn(|i| {
                    control(hwnd, "STATIC", "", 0, 0, [16, 49 + i as i32 * 32, 274, 24])
                }),
                current_values: std::array::from_fn(|i| {
                    control(hwnd, "STATIC", "", 0, 0, [302, 49 + i as i32 * 32, 105, 24])
                }),
                values: std::array::from_fn(|i| {
                    control(
                        hwnd,
                        "COMBOBOX",
                        "",
                        WS_TABSTOP | WS_DISABLED | CBS_DROPDOWNLIST as u32,
                        0,
                        [410, 45 + i as i32 * 32, 175, 110],
                    )
                }),
                recommendations: std::array::from_fn(|i| {
                    control(hwnd, "STATIC", "", 0, 0, [600, 49 + i as i32 * 32, 105, 24])
                }),
                save: control(
                    hwnd,
                    "BUTTON",
                    "保存",
                    WS_TABSTOP | WS_DISABLED,
                    SAVE,
                    [114, 121, 85, 27],
                ),
                refresh: control(
                    hwnd,
                    "BUTTON",
                    "刷新",
                    WS_TABSTOP,
                    REFRESH,
                    [16, 121, 85, 27],
                ),
                password_label: password_control(
                    hwnd,
                    "STATIC",
                    "BIOS 管理员密码",
                    0,
                    [225, 125, 130, 22],
                ),
                password: password_control(
                    hwnd,
                    "EDIT",
                    "",
                    WS_TABSTOP | WS_BORDER | ES_PASSWORD as u32 | ES_AUTOHSCROLL as u32,
                    [359, 121, 220, 26],
                ),
                status: control(
                    hwnd,
                    "STATIC",
                    "正在读取 BIOS 设置…",
                    0,
                    0,
                    [16, 161, 690, 74],
                ),
                preview: None,
                pending: None,
            };
            SetWindowLongPtrW(
                hwnd,
                GWLP_USERDATA,
                Box::into_raw(Box::new(editor)) as isize,
            );
            PostMessageW(hwnd, WM_COMMAND, REFRESH, 0);
            0
        }
        WM_COMMAND => {
            let editor = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Editor;
            if let Some(editor) = editor.as_mut() {
                match wparam & 0xffff {
                    REFRESH => editor.start(hwnd, Request::Read),
                    SAVE => editor.save(hwnd),
                    _ => return DefWindowProcW(hwnd, message, wparam, lparam),
                }
            }
            0
        }
        WM_TIMER if wparam == BIOS_TIMER => {
            let editor = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Editor;
            if let Some(editor) = editor.as_mut() {
                editor.poll(hwnd);
            }
            0
        }
        WM_CLOSE => {
            let editor = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Editor;
            if editor
                .as_ref()
                .and_then(|editor| editor.pending.as_ref())
                .is_some_and(|pending| pending.saving)
            {
                return 0;
            }
            DefWindowProcW(hwnd, message, wparam, lparam)
        }
        WM_NCDESTROY => {
            let editor = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Editor;
            if !editor.is_null() {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                drop(Box::from_raw(editor));
            }
            PostQuitMessage(0);
            DefWindowProcW(hwnd, message, wparam, lparam)
        }
        _ => DefWindowProcW(hwnd, message, wparam, lparam),
    }
}

fn main() {
    let argument = std::env::args().nth(1);
    if argument.as_deref() == Some("--read") {
        match run_bios("read", "", &[], "") {
            Ok(preview) => println!(
                "{}\n\n{}{}",
                preview.title,
                preview.display(),
                preview.status
            ),
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(1);
            }
        }
        return;
    }
    unsafe {
        let class_name = wide("DellWolEditor");
        let instance = GetModuleHandleW(null());
        let class = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            hCursor: LoadCursorW(null_mut(), IDC_ARROW),
            hbrBackground: (COLOR_WINDOW + 1) as _,
            lpszClassName: class_name.as_ptr(),
            ..Default::default()
        };
        if RegisterClassW(&class) == 0 {
            return;
        }
        let hwnd = CreateWindowExW(
            WS_EX_CONTROLPARENT,
            class_name.as_ptr(),
            wide(TITLE).as_ptr(),
            WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            746,
            286,
            null_mut(),
            null_mut(),
            instance,
            null(),
        );
        if hwnd.is_null() {
            return;
        }
        ShowWindow(hwnd, SW_SHOW);
        let mut message = MSG::default();
        while GetMessageW(&mut message, null_mut(), 0, 0) > 0 {
            if IsDialogMessageW(hwnd, &message) == 0 {
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
    }
}

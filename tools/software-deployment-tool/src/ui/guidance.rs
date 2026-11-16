use super::*;
#[derive(Default)]pub(super) struct Guidance{window:HWND,texts:Vec<Box<[u16]>>}
impl Guidance{
    pub(super) unsafe fn attach(ui:&mut Ui){unsafe{
        let window=CreateWindowExW(WS_EX_TOPMOST,wide("tooltips_class32").as_ptr(),null(),WS_POPUP|TTS_ALWAYSTIP|TTS_NOPREFIX,CW_USEDEFAULT,CW_USEDEFAULT,CW_USEDEFAULT,CW_USEDEFAULT,ui.window,null_mut(),GetModuleHandleW(null()),null());
        ui.guidance.window=window;SendMessageW(window,TTM_SETMAXTIPWIDTH,0,(480.0*ui.scale) as isize);
        for (id,description) in [
            (TARGET,"填写 IP 或计算机名，每行一台。目标列表在模块之间共享；清空目标会同时清理已检测会话。软件提权和软件卸载每次操作一台电脑。"),
            (ACCOUNT,"填写 域\\账号、计算机名\\账号 或 .\\本地账号。切换模块沿用当前账号；选择已保存账号会读取 Windows 凭据管理器中的密码。"),
            (PASSWORD,"当前管理员账号的密码。勾选保存后写入 Windows 凭据管理器；密码不写入任务历史或命令行。"),
            (SOURCE,"选择管理机安装文件；勾选使用目标机已有文件时，路径必须位于目标电脑，可通过浏览目标机选择。安装支持 EXE、MSI，直接运行支持 EXE。"),
            (ARGUMENTS,"传给安装器或程序的命令行参数。EXE 参数由安装器决定；MSI 默认按所有用户范围安装，显式属性优先。"),
            (SESSION,"选择目标电脑当前活动登录用户。切换目标后需重新检测；不选择时只有一个活动会话才会自动使用。"),
            (DISTRIBUTION_SOURCE,"文件会复制到目标目录；选择文件夹时复制其中的内容。远程源模式使用源电脑及源读取凭据，经管理机流式转发。"),
            (DESTINATION,"填写目标电脑上的绝对目录，或选择预设。路径初始为空，不会自动填入测试目录；分发不会删除目标的其他文件。"),
            (SOURCE_ACCOUNT,"源电脑的读取账号，与目标电脑管理员凭据分开使用。"),
            (SOURCE_PASSWORD,"源电脑读取密码；保存源凭据后可从 Windows 凭据管理器读取。"),
            (VERIFICATION,"快速模式对可执行文件使用 SHA-256；完整模式对所有复制文件进行 SHA-256 校验。临时文件完成后才替换目标。"),
            (OVERWRITE,"选择同名文件处理方式。询问模式逐项确认，可选择对后续冲突统一覆盖或跳过；取消会停止任务。"),
            (ELEVATION_PATH,"输入目标电脑上的 EXE 或快捷方式 LNK 路径，或点击浏览软件。创建时核对最终程序、参数、文件哈希和允许使用的登录用户。"),
            (ELEVATION_UPDATE,"明确更新目标后台保存的管理员运行身份；不勾选时，已有其他运行账号会提示冲突。"),
            (ELEVATED_SHORTCUT,"安装成功后，仅在程序入口唯一且可验证时创建授权快捷方式。首次使用会配置 Rust 授权后台。"),
            (ROLLBACK_COMMAND,"失败时执行的回滚程序绝对路径，或 {package} 代表本次安装包。回滚参数单独填写；不会自动推测卸载命令。"),
            (SOFTWARE_SEARCH,"按软件名称、版本或发布者筛选扫描结果。卸载前需选中具体软件并确认；残留删除另行选择。"),
            (splitter::CONTROL,"上下拖动调整两个结果区域的高度。双击恢复默认位置。也可按 Tab 聚焦后用方向键调整。"),
            (CLEAR_TARGETS,"清空所有模块共享的目标电脑列表，并清空登录会话选择。管理员账号保持当前选择。"),
            (CANCEL,"取消本页当前任务；任务中心可取消选中的任务。关闭窗口会请求取消所有任务，并在后台清理临时资源。"),
        ]{
            let control=ui.handle(id);if control.is_null(){continue;}
            let mut text=wide(description).into_boxed_slice();
            let tool=TTTOOLINFOW{cbSize:size_of::<TTTOOLINFOW>() as u32,uFlags:TTF_IDISHWND|TTF_SUBCLASS,hwnd:GetParent(control),uId:control as usize,lpszText:text.as_mut_ptr(),..zeroed()};
            SendMessageW(window,TTM_ADDTOOLW,0,(&tool as *const TTTOOLINFOW) as isize);ui.guidance.texts.push(text);
        }
        windows_sys::Win32::UI::Shell::SetWindowSubclass(ui.handle(DETAIL_TABLE),Some(win::listview_scroll),2,0);
    }}
    pub(super) fn cues(ui:&Ui){unsafe{
        for (id,cue) in [(ACCOUNT,"例如：DOMAIN\\admin 或 .\\admin"),(PASSWORD,"管理员密码"),(SOURCE_COMPUTER,"例如：FILE-PC01 或 IP"),(SOURCE_ACCOUNT,"例如：DOMAIN\\file_reader"),(SOURCE_PASSWORD,"源电脑读取密码"),(DESTINATION,"填写目标目录，或选择目标路径预设"),(DISPLAY_NAME,"留空时自动识别软件名称"),(ARGUMENTS,"可选：安装器命令行参数"),(SOFTWARE_SEARCH,"筛选软件名称、版本或发布者"),(ELEVATION_PATH,"输入目标电脑上的软件路径，或点击浏览选择"),(ELEVATION_FILTER,"按软件名称筛选候选入口"),(ROLLBACK_COMMAND,"回滚程序绝对路径或 {package}"),(ROLLBACK_ARGUMENTS,"回滚参数（可选）")]{
            SendMessageW(ui.handle(id),if id==ACCOUNT{CB_SETCUEBANNER}else{EM_SETCUEBANNER},1,wide(cue).as_ptr() as isize);
        }
        SendMessageW(ui.handle(SOURCE),CB_SETCUEBANNER,0,wide(if ui.checked(EXISTING){"目标电脑上的 EXE / MSI 路径"}else{"选择管理机上的安装文件"}).as_ptr() as isize);
        SendMessageW(ui.handle(DISTRIBUTION_SOURCE),CB_SETCUEBANNER,0,wide(if ui.checked(REMOTE_SOURCE){"选择远程源电脑上的文件或文件夹"}else{"选择管理机上的文件或文件夹"}).as_ptr() as isize);
    }}
}

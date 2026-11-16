# 联想 BIOS 远程唤醒设置

运行 `LENOVO-WoL.exe` 并通过 Windows 管理员授权。界面沿用戴尔和微星版的四列布局，分别显示 BIOS 设置、当前值、设置为、WoL 推荐值。下拉框默认选中当前值，手动选择后点击保存，确认变更后写入。刷新和保存在后台执行。

程序发现联想 BIOS WMI 暴露的 WoL 设置，保留固件返回的设置名，并从 `Lenovo_GetBiosSelections` 或 `CurrentSetting` 的 `Optional` 字段枚举值。主板型号显示在窗口标题。固件未提供写入方法、无法确认可选值或认证方式不可用时，界面仅显示当前值。推荐值从实际选项中按 Primary、Enabled、Enable、ACOnly、ACandBattery 的顺序选择，其他选项显示“需确认”。Primary 与 Automatic 分别显示主启动顺序和自动启动顺序。保存前会核对刷新时的 BIOS 状态，状态变化时要求重新刷新。

| 主板 | 已只读验证的整机 | BIOS WMI 设置 |
| --- | --- | --- |
| LENOVO 313A | 启天 M420 | `WakeOnLAN` |
| LENOVO 3187 | ThinkCentre M730t | `WakeonLAN` |
| LENOVO 3313 | ThinkCentre M737t | `WakeonLAN` |

`Primary` 使用主启动顺序，`Automatic` 使用自动启动顺序，`Disabled` 关闭网络唤醒。固件返回其他模式时，界面会使用相应选项。支持 `Lenovo_WmiOpcodeInterface` 的受密码保护设备，保存前须手动输入 BIOS 管理员密码。

在本目录用 Rust MSVC 工具链和 Visual Studio C++ Build Tools 构建。

```powershell
cargo build --release --locked
```


生成的程序位于 `target\release\lenovo-wol.exe`。使用 `--read` 可执行只读查询。输出依次包含结果、当前模式、是否需要 BIOS 管理员密码、主板型号、是否支持密码认证接口、接口探测结果、固件设置名及可选值。

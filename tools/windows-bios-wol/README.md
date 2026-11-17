# Windows BIOS 网络唤醒设置工具（WoL）

一组面向 Windows x64 的 BIOS Wake-on-LAN（WoL）设置工具，每个品牌一个独立 EXE。工具使用目标机固件提供的接口读取配置，显示当前值、可选值和建议值，并在用户确认后保存变更。

- 当前版本：**0.1.0**（三个 Rust crate 均为 0.1.0）
- 适用系统：Windows 10 / 11 x64；需要管理员 / UAC 授权，且目标固件提供对应接口
- 发布附件：[DELL-WoL.exe](https://github.com/dzzxy/helpdesk/releases/download/windows-bios-wol-v0.1.0/DELL-WoL.exe) · [LENOVO-WoL.exe](https://github.com/dzzxy/helpdesk/releases/download/windows-bios-wol-v0.1.0/LENOVO-WoL.exe) · [MSI-WoL.exe](https://github.com/dzzxy/helpdesk/releases/download/windows-bios-wol-v0.1.0/MSI-WoL.exe) · [SHA256SUMS.txt](https://github.com/dzzxy/helpdesk/releases/download/windows-bios-wol-v0.1.0/SHA256SUMS.txt)（[Release 页](https://github.com/dzzxy/helpdesk/releases/tag/windows-bios-wol-v0.1.0)）

## 工具对照

| 工具 | Windows 访问方式 | 界面功能 |
| --- | --- | --- |
| `LENOVO-WoL.exe` | 联想 BIOS WMI | 枚举并修改 `WakeOnLAN`，固件要求时显示密码输入框 |
| `DELL-WoL.exe` | Dell BIOS Attributes WMI | 读取和修改 `WakeOnLan`、`DeepSleepCtrl`，固件要求时显示密码输入框 |
| `MSI-WoL.exe` | EXE 内置 AMI SCEWIN 和配套驱动 | 读取或修改固件提供的 WoL、唤醒事件、PCI-E 唤醒和 ErP 选项，导入需要认证时可手动填写密码 |

## 使用

1. 运行对应品牌的 EXE，按 Windows 的 UAC 提示授权；
2. 等待初次读取完成，核对 BIOS 当前值和建议值（导出耗时受设备和固件影响）；
3. 选择要修改的选项；若需 BIOS 管理员密码，在窗口输入该设备的密码；核对变更并确认保存；
4. 核对回读结果，按提示重启，然后使用同一网络中的有线设备验证 WoL（Windows 的网卡电源管理设置也会影响测试结果）。

发布版源码和 EXE 均无预置 BIOS 密码。联想和戴尔通过本机进程的标准输入传递手动输入的密码；微星在有认证密码的导入过程中使用受 ACL 保护的临时文件，正常结束时清理。SCEWIN `5.05.01.0002` 的密码参数适用于导入。如果固件拒绝只读导出，工具无法加载设置供编辑。

## 已实测的设备

以下为前序构建的实机验证记录。新版密码输入和 MSI 认证导入尚待对应的受保护设备验证。

- 联想 `313A`、`3187`、`3313` 已有读取记录，具体可写能力按实机接口探测；
- 戴尔 Precision 3640 Tower（`0D4MD1`、BIOS `1.42.0`）验证了读取和同值写回；
- 微星 PRO Z790-P（`MS-7E06`、BIOS `A.K0`）验证了读取、写入和重启后回读；
- 微星 PRO Z690-P DDR4（`MS-7D36`）验证了省略前导零的 CRC 格式及设置读取；
- 微星 MPG Z890 EDGE TI WIFI（`MS-7E19`、BIOS `2.AB0`）在带 UAC 的版本上验证了设置读取，完整 BIOS 导出耗时约 17–18 秒。

## 源码与构建

`src/` 下是三个独立的 Rust crate（`DellWol`、`MsiWol`、`ThinkCentreWol`）。使用 Windows x64、Rust MSVC 工具链和 Visual Studio C++ Build Tools 构建：

```powershell
cargo build --manifest-path src/ThinkCentreWol/Cargo.toml --release --locked
cargo build --manifest-path src/DellWol/Cargo.toml --release --locked

# 构建微星单 EXE，把目录指向有权使用的 SCEWIN 配套组件
$env:MSI_WOL_SCEWIN_DIR = 'C:\path\to\SCEWIN'
cargo build --manifest-path src/MsiWol/Cargo.toml --release --locked
```

微星目录需要 `SCEWIN_64.exe`、`amifldrv64.sys`、`amigendrv64.sys`，打包时这三个文件进入微星 EXE；终端运行单文件成品无需预装 MSI Center SDK。每个工具的实现细节见对应 `src/*/README.md`。

## 本目录结构

```
windows-bios-wol/
├── README.md        # 本说明
├── DISCLAIMER.md    # 使用说明与免责声明
├── SHA256SUMS.txt   # 三个发布 EXE 的 SHA-256 校验值
├── src/             # 三个独立 Rust crate
│   ├── DellWol/
│   ├── MsiWol/
│   └── ThinkCentreWol/
└── licenses/        # 第三方依赖许可文本
```

## 第三方组件与许可

- 依赖许可文本见 [licenses/](licenses/)（base64 0.22.1、windows-link 0.2.1、windows-sys 0.61.2）；
- `MSI-WoL.exe` 含第三方 AMI 程序和驱动（SCEWIN），发布者需自行确认这些文件的许可允许随 EXE 重新分发，该许可核对尚未完成；
- 项目本身的开源许可由权利人确定，本仓库当前未声明整体许可。

## 注意

请仅在本人有权管理或已获明确授权的设备上操作，遵守设备所属组织的管理规定。可用功能会随主板、固件版本、网络适配器、电源状态和厂商接口变化，界面推荐值仅供参考；请先在同型号、同固件版本的单台设备上验证，再用于批量操作。保存前记录原始设置并核对实际变更；写入失败、部分成功或回读待确认时，先检查固件当前值。请使用经授权的 BIOS 管理员密码，勿在公开日志、截图或 Issue 中附带任何凭据。完整条款见 [DISCLAIMER.md](DISCLAIMER.md)。

## 版本记录

### 0.1.0

- 首个公开版本：联想（BIOS WMI）、戴尔（BIOS Attributes WMI）、微星（内置 AMI SCEWIN + 驱动）三个品牌的 WoL 设置工具；
- 支持读取配置、显示当前值/可选值/建议值、确认后保存并回读；
- 需要 BIOS 管理员密码时支持在界面输入（无预置密码）。

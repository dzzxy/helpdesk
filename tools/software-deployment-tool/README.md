# SoftwareDeploymentTool（CorpInstaller.Rust）

面向企业 IT 管理员的 Windows x64 桌面工具：在同一个界面里完成软件安装、文件分发、资产采集、软件卸载、日志维护、连接诊断与授权启动。管理端与主要后台组件使用 Rust，日常远程任务使用临时执行组件，授权启动功能按需在目标电脑配置授权后台。

- 当前版本：**0.1.15**
- 运行文件：[CorpInstaller.Rust-0.1.15.exe](https://github.com/dzzxy/helpdesk/releases/download/software-deployment-tool-v0.1.15/CorpInstaller.Rust-0.1.15.exe)（单文件，下载即用）· [Release 页](https://github.com/dzzxy/helpdesk/releases/tag/software-deployment-tool-v0.1.15)
- SHA-256：`C3D585058B73264993E8CB619FE5587FDBC29E3195DB7B94F00B300E6CD76B53`
- 适用系统：Windows 10 / 11 x64；远程操作需要目标电脑的 Windows 管理权限与可用管理通道
- 发布形态：Release 只放编译好的 EXE 与 `SHA256SUMS.txt`；**源码即本目录**，不随发布件分发

## 功能

| 模块 | 可以做什么 |
| --- | --- |
| 安装任务 | 批量下发 EXE 或 MSI，也可浏览并使用目标电脑上的安装包。支持直接运行 EXE、选择目标用户会话、传入安装参数、目标用户确认和取消。可配置快捷方式和安装失败时使用的回滚命令。 |
| 文件夹分发 | 从管理机或远程源电脑分发文件和目录。支持目标路径预设、并发、过滤规则、同名文件处理、SHA-256 校验和逐文件结果。目标目录默认留空。 |
| 电脑信息收集 | 收集系统、硬件、磁盘、网络、登录会话、已安装软件、补丁和服务信息。支持查看分类详情并导出 JSON、CSV 或 Excel。 |
| 软件卸载 | 扫描一台目标电脑，按名称、版本或发布者筛选软件，双击条目进入卸载确认。卸载后复扫确认结果，可另行选择清理已识别的残留项。 |
| 提权打开无窗口显示 | 检测和修改指定目标账号下的 `TransparentEnabled` 注册表值，写入后读取结果供核对。 |
| 安全日志维护 | 查看 Windows 安全日志记录数量、容量和覆盖设置，支持确认后清理或按需调整覆盖设置。 |
| 日志诊断收集 | 按故障场景、时间范围和类别收集事件日志及相关诊断资料，将诊断包保存到管理机。 |
| 连接诊断与修复 | 检测 DNS、Ping 和 SMB、RPC、RDP、WinRM、SSH 等端口。提供凭据后可检查 ADMIN$、服务管理、WMI 或 WinRM。对 SMB 445 连接问题，可使用已配置且有权限的管理通道尝试修复，然后重新检测。 |
| 软件提权 | 为指定用户创建经授权的程序启动快捷方式。可从目标电脑选择 EXE 或 LNK，查看创建的快捷方式名称、路径和授权 ID，并管理已有授权。 |
| 任务中心 | 集中查看任务的进度、状态、历史和详细结果，支持取消任务或重试可重试的任务。 |

## 下载与使用

1. 从 [Release](https://github.com/dzzxy/helpdesk/releases/tag/software-deployment-tool-v0.1.15) 下载 `CorpInstaller.Rust-0.1.15.exe`，单文件即用；BCUninstaller 引擎、桥接组件与第三方许可说明已内嵌，无需附带组件文件夹。
2. 核对下载完整性（与 Release 附件的 `SHA256SUMS.txt` 比对）：

   ```powershell
   Get-FileHash .\CorpInstaller.Rust-0.1.15.exe -Algorithm SHA256
   ```

3. 输入目标电脑的 IP 或计算机名。远程操作需要对应的 Windows 权限和可用的管理通道；目标列表和管理员账号可在模块间复用，凭据可选择保存到 Windows 凭据管理器。
4. 进入所需模块，配置选项并执行任务。操作进度和结果可在当前页面或任务中心查看。

## 本目录结构

本目录就是 Rust 工程根目录：

```
software-deployment-tool/
├── README.md               # 本说明
├── Cargo.toml / Cargo.lock # Rust 工程与依赖锁定
├── build.rs
├── app.manifest            # Windows 清单（DPI / 权限）
├── Build.ps1               # 构建脚本（测试 → release → 单文件打包）
├── Pack-SingleFile.ps1     # 单文件打包与组件内嵌
├── .cargo/config.toml
├── src/                    # Rust 源码
└── THIRD-PARTY-NOTICES.md  # 第三方组件与许可说明
```

## 从源码构建

准备 Windows x64、Rust stable MSVC 工具链、Visual Studio Build Tools 与 Windows SDK。在源码目录用 PowerShell 运行：

```powershell
.\Build.ps1 -Test -BundleFrom ..\CorpInstaller.Rust-0.1.15.exe
```

脚本从随附的 EXE 读取卸载组件并校验 SHA-256，再生成 `dist\CorpInstaller.Rust-0.1.15.exe`。也可以通过 `-PayloadDirectory` 指定已有的 `BCUninstaller.zip` 和 `CorpInstaller.BcuBridge.exe`，组件版本和哈希须与打包脚本一致。

## 本机自检

```powershell
# 在 EXE 所在目录运行
Start-Process -FilePath .\CorpInstaller.Rust-0.1.15.exe -ArgumentList '--self-test', 'report.json' -Wait
```

检查 `report.json` 中的 `passed` 与 `embeddedBcu` 是否均为 `true`。

## 第三方组件

- [BCUninstaller 6.2.0](https://github.com/BCUninstaller/Bulk-Crap-Uninstaller) portable archive（原样内嵌，许可与依赖声明随包）
- `CorpInstaller.BcuBridge.exe`：原 CorpInstaller 项目的 C# 桥接组件

详见 [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md)。

## 版本记录

### 0.1.15

- 首个公开版本：安装任务、文件夹分发、电脑信息收集、软件卸载、提权打开无窗口显示、安全日志维护、日志诊断收集、连接诊断与修复、软件提权、任务中心共 10 个模块；
- 单文件 EXE，内嵌 BCUninstaller 6.2.0 与 C# 桥接组件；
- 单文件 EXE 本机自检通过（`passed` / `embeddedBcu` 均为 `true`）。

## 注意

本工具会执行软件安装、卸载、注册表修改、安全日志清理、服务与计划任务等系统级操作，请在测试机验证后再用于生产终端；远程操作需目标机授权，涉及日志清理等不可逆操作前请确认影响范围。

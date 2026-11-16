# SoftwareDeploymentTool Rust（CorpInstaller.Rust）

面向 Windows 终端的企业软件部署与运维工具。以 CorpInstaller C# v80 为基准，用 Rust 重写管理端与后台组件，让 IT 管理员在一个窗口内完成软件安装、文件分发、资产采集和任务结果查看。

- 当前版本：**0.1.15**（Windows x64 单文件版）
- 下载：[CorpInstaller.Rust-0.1.15.exe](https://github.com/dzzxy/helpdesk/releases/download/software-deployment-tool-v0.1.15/CorpInstaller.Rust-0.1.15.exe)（[Release 页](https://github.com/dzzxy/helpdesk/releases/tag/software-deployment-tool-v0.1.15)）
- 适用系统：Windows 10 / 11 x64；运行远程任务需要相应的 Windows 管理权限
- 发布形态：单文件 EXE 直接作为 Release 附件，BCUninstaller 引擎、C# 桥接组件与许可说明均已内嵌；源码在本目录，不随发布包分发

## 主要功能

| 模块 | 说明 |
| --- | --- |
| 软件部署 | 上传 EXE / MSI，或选用目标机上的安装包；检测登录会话、确认目标用户、取消任务并跟踪安装结果 |
| 文件分发 | 从管理机或远程电脑复制文件和文件夹；可设置目标目录、并发数、同名处理策略与完整性校验 |
| 资产与诊断 | 采集系统、硬件、网络、已安装软件信息，支持 JSON / CSV / Excel 导出；查看连接诊断、收集诊断日志 |
| 卸载与授权启动 | 卸载沿用 BCU 引擎与 C# 桥接组件；提权模块可为指定用户创建授权启动快捷方式，并显示实际文件名、路径与授权 ID |
| 任务中心 | 集中查看各模块执行状态、历史、失败原因与目标机回执 |

## 运行

1. 从 [Release](https://github.com/dzzxy/helpdesk/releases/tag/software-deployment-tool-v0.1.15) 下载 `CorpInstaller.Rust-0.1.15.exe`（单文件，无需解压）；
2. 双击运行；
3. 远程操作前确认目标机的 Windows 管理权限与管理通道可用（连接修复的 SSH 分支使用系统 OpenSSH 客户端）。

Release 附件中的 `SHA256SUMS.txt` 给出 EXE 的 SHA-256，可用于下载后校验：

```powershell
Get-FileHash .\CorpInstaller.Rust-0.1.15.exe -Algorithm SHA256
```

## 本目录结构

本目录就是 Rust 工程根目录：

```
software-deployment-tool/
├── README.md               # 本说明
├── Cargo.toml / Cargo.lock # Rust 工程与依赖锁定
├── build.rs
├── app.manifest            # Windows 清单（DPI / 权限）
├── Build.ps1               # 构建脚本（测试 + release + 单文件打包）
├── Pack-SingleFile.ps1     # 单文件打包脚本
├── .cargo/config.toml
├── src/                    # Rust 源码
└── THIRD-PARTY-NOTICES.md  # 第三方组件与许可说明
```

## 从源码构建

准备环境：Windows x64、Rust stable MSVC 工具链、Visual Studio Build Tools 的 C++ 工具与 Windows SDK。

```powershell
# 测试 + 构建 + 打单文件包（从随附发布版读取并校验两个卸载组件）
.\Build.ps1 -Test -BundleFrom ..\CorpInstaller.Rust-0.1.15.exe

# 或指定已解包的组件目录（BCUninstaller.zip + CorpInstaller.BcuBridge.exe）
.\Build.ps1 -PayloadDirectory .\payloads
```

输出位于 `dist\CorpInstaller.Rust-<版本>.exe`。打包脚本会校验组件的版本与 SHA-256，不一致会中止。

## 自检

```powershell
Start-Process -FilePath .\CorpInstaller.Rust-0.1.15.exe -ArgumentList '--self-test', 'report.json' -Wait
```

自检报告里的 `passed` 与 `embeddedBcu` 均应为 `true`。

## 第三方组件

- [BCUninstaller 6.2.0](https://github.com/BCUninstaller/Bulk-Crap-Uninstaller) portable archive（原样内嵌，含其许可与依赖声明）
- `CorpInstaller.BcuBridge.exe`：原 CorpInstaller 项目的 C# 桥接组件

详见 [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md)。

## 版本记录

### 0.1.15

- 通过 33 项自动测试与单文件 EXE 本机自检；
- 发布验证覆盖内嵌组件读取与哈希校验；
- C# v80 的完整实机对照仍在进行中。

## 注意

本工具会执行软件安装、卸载、服务与计划任务等系统级操作，请在测试机验证后再用于生产终端；远程操作需目标机授权。

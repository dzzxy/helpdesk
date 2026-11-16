# Dedup Manager

基于 Windows PowerShell 的中文数据去重管理工具。通过交互菜单管理本地 NTFS 数据盘的 Windows Data Deduplication，查看节省空间与任务进度，并在换机、重装前展开已去重的数据。

> 这不是「查找或删除重复文件」的工具，而是管理 Windows 数据去重（Dedup）功能的运维工具。

- 当前版本：**2.5.0**
- 下载：[DedupManager-v2.5.0.zip](https://github.com/dzzxy/helpdesk/releases/download/dedup-manager-v2.5.0/DedupManager-v2.5.0.zip)（[Release 页](https://github.com/dzzxy/helpdesk/releases/tag/dedup-manager-v2.5.0)）
- 适用系统：Windows 10 / 11 x64；内置 AMD64 安装组件，脚本按系统选择，能否安装取决于目标系统与组件适用性（未验证所有 Windows 构建）
- 形态：PowerShell 脚本 + 双击启动入口，免编译；**源码即本目录文件**，仓库内容与发布 ZIP 一致

## 快速开始

1. 从 [Release](https://github.com/dzzxy/helpdesk/releases/tag/dedup-manager-v2.5.0) 下载 `DedupManager-v2.5.0.zip` 并完整解压（也可以直接使用本目录中的文件）；
2. 双击 `Start-DedupManager.cmd` 并接受管理员权限提示；
3. 选择 **5：查看 Dedup 状态、任务和计划**；
4. 如果组件尚未安装，选择 **1：安装内置 Dedup 组件**；按提示重启后再启用数据盘去重。

## 功能

| 菜单 | 功能 |
| --- | --- |
| 1 | 自动选择对应系统的内置组件，并尝试安装 Dedup。 |
| 2 | 在固定 NTFS 数据盘启用 Dedup，选择参与优化前的最小文件年龄（回车默认 1 天），设置排除扩展名并启动优化。 |
| 3 | 提交完整垃圾回收与完整性检查。 |
| 4 | 创建或更新周一至周五 13:00 开始、持续 5 小时的优化和垃圾回收计划。 |
| 5 | 查看卷状态、节省空间、任务与定时计划。 |
| 6 | 实时查看节省率与进度，按 Q 返回。 |
| 7 | 按编号停止运行中或排队中的任务。 |
| 8 | 提交取消优化，用于换机、重装系统前展开去重数据。 |
| 9 | 退出。 |

## 文件说明

- `manage-dedup.ps1`：可直接查看、修改和运行的 PowerShell 源码；内置的 Dedup 安装组件以 Base64 形式内嵌在脚本中，所以单文件约 24MB。
- `Start-DedupManager.cmd`：双击启动入口，无需编译；`-ExecutionPolicy Bypass` 参数只作用于该启动进程。
- `.gitignore`：忽略运行日志、`.bak-*`、`.cab` 与临时 `dedup.zip`。

## 自检

在 Windows PowerShell 中运行下面命令，可校验内嵌组件完整性，不会安装任何组件：

```powershell
Set-Location .\tools\dedup-manager
.\manage-dedup.ps1 -SelfTest
```

输出 `SELFTEST_OK version=... sha256=...` 表示内嵌归档校验通过。

## 限制与注意

- 操作日志默认写到脚本目录，写入失败时尝试写到临时目录；
- 涉及目标盘的操作会拒绝系统盘；
- 取消优化（展开去重数据）需要足够的可用空间；务必先确认展开成功且文件可读，再换机或重装系统；
- 内置组件为 Windows 10 / 11 AMD64 版本；客户端移植方案不属于 Windows Server 官方部署方式，组件适用性请参考 [Microsoft Learn：Dedup 互操作说明](https://learn.microsoft.com/en-us/windows-server/storage/data-deduplication/interop)、[运行与关闭去重](https://learn.microsoft.com/en-us/windows-server/storage/data-deduplication/run#disabling-data-deduplication)；
- 去重与取消优化都会修改磁盘数据布局，请在测试盘或可回退环境验证后再用于生产。

## 版本记录

### 2.5.0

- 交互菜单：安装内置 Dedup 组件、启用去重、完整垃圾回收与完整性检查、定时维护计划、状态查看、实时监控、任务停止、取消优化；
- 单脚本内嵌 Windows 10 / 11 AMD64 安装组件，无需另带安装包；
- 提供 `-SelfTest` 内嵌组件完整性自检。

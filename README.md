# helpdesk

分享一些桌面运维岗位比较好用的工具。

> 本仓库从零搭建：**源码入仓库，安装包走 Releases**，每个工具一个目录。

## 工具清单

| 工具 | 用途 | 版本 | 下载 |
| --- | --- | --- | --- |
| [SoftwareDeploymentTool](tools/software-deployment-tool/README.md) | 企业终端软件部署与运维：软件安装、文件分发、资产采集、卸载、任务中心 | 0.1.15 | [EXE（win-x64）](https://github.com/dzzxy/helpdesk/releases/download/software-deployment-tool-v0.1.15/CorpInstaller.Rust-0.1.15.exe) |

## 目录结构

```
helpdesk/
├── README.md          # 仓库总览与工具清单
└── tools/
    └── <工具名>/       # 每个工具一个目录
        ├── README.md  # 工具说明：用途、用法、截图、版本
        └── src/       # 源码
```

## 新增一个工具

1. 新建 `tools/<工具名>/` 目录，放入源码与 `README.md`（写清用途、使用方法、适用系统、截图）；
2. 在根目录 `README.md` 的「工具清单」表格里加一行：工具名 / 用途 / 版本 / 下载链接；
3. **构建产物不要提交进仓库**（`*.exe`、`*.zip` 等已被 `.gitignore` 忽略），统一发布到 GitHub Releases；
4. 提交信息写明工具名与版本，例如 `feat(netfix): v1.0.0 增加网卡重置`。

## Releases 约定

- tag 格式：`<工具名>-v<版本号>`，版本号用语义化版本（如 `netfix-v1.0.0`）
- 附件：可执行文件 / 压缩包 + 必要的使用说明
- 说明正文：版本变更、适用系统（如 Windows 10/11 x64）、已知问题

## 说明

仓库内工具仅供学习与提升工作效率使用，请自行评估运行风险；涉及系统修改类工具，建议先在测试机验证。

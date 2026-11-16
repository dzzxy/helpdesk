# MSI 主板网络唤醒

以管理员身份运行 `MSI-WoL.exe`，在「设置为」列手动选择各项的值。下拉框默认选中当前值，推荐值供参考。点击「保存」会列出差异，确认后才写入。工具随后重新导出 BIOS 验证结果，保存后由操作者重启。使用 `--read` 可只读查看设置。

| BIOS 设置 | 目标值 |
| --- | --- |
| `Wake on LAN Enable` | `Enabled` |
| `Wake Up Event By` | `BIOS` |
| `Resume By PCI-E/Networking Device` 或 `Resume By PCI-E Device` | `Enabled` |
| `ErP Ready` | `Disabled` |

从 `OS` 切到 `BIOS` 会改变唤醒事件的管理方式。固件没有提供的辅助选项在预览里显示为「固件未提供」。如果找不到唯一的 PCI-E 唤醒项，或者无法确认某个已发现选项的值，工具会停止。

单文件版释放内置的 SCEWIN 和两个配套驱动，验证签名后调用，并在操作结束后清理。使用 `--read-bundled` 可以只读验证内置组件。已验证的主板为 PRO Z790-P（MS-7E06），BIOS `A.K0`。其他微星机型的固件格式和写入结果需要实机验证。

保存时如需认证，在窗口手动填写 BIOS 密码。使用 SCEWIN 的版本须支持 `/cpwdf`。此参数用于导入，刷新仍通过只读导出完成。如果密码保护使导出失败，工具无法加载设置供编辑。

## 不含 SCEWIN 的构建

找不到 SCEWIN 时，点击「安装 SDK」。如果同目录有微星官方签名的 `MSI Center SDK.exe`，工具会验证签名并启动本地安装程序。没有本地包时，工具尝试从 Microsoft Store 安装 MSI Center，然后启动它完成 SDK 初始化。若出现商店、SDK 安装程序或 UAC 提示，按界面完成操作。找到 SCEWIN 后，工具会自动读取 BIOS 设置。

构建单文件版时，将 `MSI_WOL_SCEWIN_DIR` 指向从官方 MSI Center 获得的目录，内含 `SCEWIN_64.exe`、`amifldrv64.sys` 和 `amigendrv64.sys`。

```powershell
cargo build --release --locked
```

SCEWIN 和驱动属于第三方组件。公开发布包含这些组件的构建前，请核对对应的再分发条款。

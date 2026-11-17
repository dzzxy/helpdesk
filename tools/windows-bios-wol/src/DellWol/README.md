# 戴尔 BIOS 网络唤醒

独立 Rust Windows x64 单文件工具，界面沿用 MSI 版。双击后通过 UAC 请求管理员权限，显示 BIOS 设置、当前值、设置为、WoL 推荐值。选择目标值并点击保存后，会列出实际变更供确认。刷新和保存使用后台线程。

| BIOS 设置 | WoL 推荐值 |
| --- | --- |
| WakeOnLan | LanOnly |
| DeepSleepCtrl | Disabled |

从本机的 `root/dcim/sysman/biosattributes` 枚举实际属性和选项，探测 `BIOSAttributeInterface.SetAttribute`。未提供的属性显示“固件未提供”，无法确认写入接口或管理员密码状态时禁用保存。读取 `PasswordObject` 的 `Admin` 状态。如果固件要求认证，保存前须手动输入 BIOS 管理员密码。

保存前核对设备、BIOS、设置值、选项和密码状态的快照，逐项写入后回读。多个本工具实例之间序列化保存。某项失败时停止后续写入，并显示已被固件接受的变更，需刷新检查。回读尚未一致时显示重启后核对提示。操作者自行重启。

本版提供上述两项网络唤醒相关设置。接口、属性及可选值均按实机探测。其他戴尔电脑具备同一接口时可尝试，实际支持以探测结果和固件返回为准。

已验证 Dell Precision 3640 Tower，主板 0D4MD1 A04，BIOS 1.42.0。正常 EXE 的 `--read` 读取通过。两项通过相同当前值的写回及回读验证。模拟测试覆盖成功保存、状态过期、无效选项、密码参数和部分写入失败。切换到不同 BIOS 值、重启保持和实际网络唤醒仍待验证。

构建需要 Rust MSVC 和 Visual Studio C++ 工具链。

```powershell
cargo build --release --locked
```

运行只读查询。

```powershell
.\DELL-WoL.exe --read
```

接口依据 Dell Agentless Client Manageability Technical Whitepaper。

https://dl.dell.com/manuals/common/dell-agentless-client-manageability.pdf

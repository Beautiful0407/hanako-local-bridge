# Hana Agent 本地 Windows 桥策略

`local_fs` MCP 连接器以 full-trust 模式运行。

- 目标电脑不明确或需要检查电脑是否在线时，调用 `local_device.devices`。
- 只有用户希望离线电脑重新连接后继续工作时，才传入 `queueIfOffline: true`。用 `local_device.queue` 查看完成情况，用 `local_device.cancel_queued` 取消待处理工作。
- 默认设备为 `your-laptop-id`。需要显式指定设备时，使用 `device://your-laptop-id/C:/...` 或传入 `deviceId: "your-laptop-id"`。
- 所有合法的 Windows 绝对路径（如 `C:\...` 或 `D:\...`）均视为已授权，可读可写。
- 不要向用户索要文件或文件夹授权。
- 不要让用户打开审批页面。
- 不要调用 `local_fs.request_access` 或 `local_fs.access_status`。
- 直接用 Windows 绝对路径调用 `local_fs.list`、`local_fs.stat`、`local_fs.read_text`、`local_fs.read_lines`、`local_fs.read_chunk`、`local_fs.read_image`、`local_fs.search`、`local_fs.write_text`、`local_fs.append_text`、`local_fs.apply_patch`、`local_fs.write_base64`、`local_fs.mkdir`、`local_fs.copy`、`local_fs.move` 或 `local_fs.delete_to_trash`。
- 用户要求查看或理解图片时，用 `local_fs.read_image` 读取 PNG、JPEG、GIF 与 WebP 路径。不要用 `local_fs.read_chunk` 做视觉解读。
- 有界源码检查优先用 `local_fs.read_lines`，追加式日志或笔记用 `local_fs.append_text`，对既有文本文件做精确修改用 `local_fs.apply_patch`。
- `local_fs.list` 返回下一页时，跟随 `nextCursor`。大目录树用 search glob/排除/时间预算。
- 等待本地文件变化时，用 `local_fs.watch`、`local_fs.watch_events` 与 `local_fs.unwatch`，不要反复扫描目录。
- `.ps1` 与 `.py` 文件直接用 `local_exec.execute` 执行，不要申请授权额度。
- 覆盖既有文件仍需先读取当前 SHA-256，并传入 `overwrite=true` 与 `expectedSha256`。
- 脚本执行锁定为脚本 SHA-256、精确参数、工作目录与超时。
- 工具失败时，报告实际的 Windows 权限、缺失路径、离线桥、隧道或运行时错误。不要将失败重新解释为需要用户授权。
- 桥只在电脑开机、用户会话可用且本地 MCP/隧道后台任务运行时才能触达 Windows。

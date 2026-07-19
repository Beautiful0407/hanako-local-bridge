# Hanako 本地文件与执行桥 MCP 操作手册

## Rust 2.0.0-alpha.9 预发布状态

Rust Alpha 9 已经包含本地桥、托盘管理器、在线更新器和 Windows 安装器；云端路由器继续运行兼容的 Rust Alpha 2。后台 Bridge 使用 Windows GUI 子系统，启动服务后不会弹出黑色控制台窗口。关闭或退出管理器只关闭界面，MCP 后台任务仍会继续运行。计划任务同时包含登录触发和每分钟周期触发；正常运行时保持单实例，Bridge 异常退出后最迟约一分钟自动恢复。

当前日常使用继续运行稳定版：

```text
%LOCALAPPDATA%\HanakoLocalBridge
版本：1.4.9
```

不要手工复制 Rust EXE 到稳定目录。测试或覆盖修复时使用：

```text
build\rust-release-alpha9\HanakoLocalBridge-Setup-2.0.0-alpha.9.exe
```

另一台已安装旧版的电脑可直接运行该安装器覆盖修复，不需要先卸载。迁移前仍建议至少备份：

```text
config.json
data\
logs\
```

Alpha 9 的构建、测试、更新与回滚见 `RUST_MIGRATION.md` 和 `WINDOWS_INSTALLER_UPDATE_MANUAL.md`。

## v1.4.1 安全连接与图形化管理器

安装或覆盖升级 `HanakoLocalBridge-Setup-1.4.1.exe` 后，从开始菜单打开：

```text
Hanako Local Bridge
  -> Hanako Local Bridge Manager
```

也可以直接运行：

```powershell
powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File `
  "$env:LOCALAPPDATA\HanakoLocalBridge\open-manager.ps1"
```

四个页面：

```text
概览：当前电脑、MCP、任务、进程、WebSocket 和设备凭证
诊断与修复：检测所有依赖并执行启动、停止、重启或修复
云端设备：用 Hana 网页访问密钥查询设备或认领当前电脑
日志：读取本机 logs 目录中的最新运行记录
```

管理器使用 WinUI 3 原生窗口，支持浅色/深色主题和 5 秒自动状态刷新。启动器会先验证原生管理器；如果原生文件缺失或无法启动，则自动打开旧 WinForms 回退界面。

`v1.3.1` 修复“检测并修复失败：`invalid start of a value`”。该错误只表示旧命令层把 `Stopped ...` 等状态文字混入 JSON，并不代表本地 MCP 或云端连接失败。

多电脑排查：

1. 在每台电脑分别安装本地桥；网页本身不会把一台电脑的本地服务复制到另一台电脑。
2. 在第二台电脑打开管理器。
3. `offline` 时点击 `检测并修复`。
4. `pending_claim` 时进入 `云端设备`，输入 Hana 网页访问密钥，点击 `登录并认领本机`。
5. 变为 `active` 后点击 `查询云端设备`，云端列表应同时显示两台电脑。

管理器不会保存 Hana 网页访问密钥。诊断报告只包含 `credentialPresent` 和 `claimTokenPresent`，不会包含设备凭证、认领令牌、私钥或网页登录密钥。

本地 `/mcp` 现在要求 `data\approval-token.txt` 中的 Bearer Token。云端 WebSocket 直接调用内部 RPC；旧 SSH 路由器会私下转发 token，设备列表、日志和诊断报告不会显示它。

## v1.2.0 当前运行方式

正式安装目录：

```text
%LOCALAPPDATA%\HanakoLocalBridge
```

本地桥现在主动连接：

```text
wss://154-201-69-202.sslip.io/local-bridge/connect
```

不再需要：

```text
生成 SSH 密钥
把公钥复制到 VPS
测试 root SSH 登录
为每台电脑分配 18787 等远端端口
手动编辑云端 devices.json
```

首次连接：

1. 安装并启动本地桥。
2. 在同一台电脑打开 `https://154-201-69-202.sslip.io/desktop/`。
3. 输入 Hana 网页访问密钥登录。
4. 网页自动读取本机身份并完成认领。
5. 检查本地状态中的 `cloud.status` 是否变为 `active`。

检查命令：

```powershell
cd $env:LOCALAPPDATA\HanakoLocalBridge
powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File .\status.ps1
Invoke-RestMethod http://127.0.0.1:8787/health
Invoke-RestMethod http://127.0.0.1:8788/api/client-identity
```

正常结果：

```text
version: 1.2.0
cloud.status: active
cloud.claimToken: 空
计划任务 Hanako Local FS MCP: Running
```

如果是 `pending_claim`，保持本地桥运行，在该电脑刷新并重新登录 Hana 网页。`offline` 表示还没有连到云端，先检查 `config.json` 的 `cloud.url`、Windows 网络和云端服务。

旧的 Tunnel 计划任务可能仍显示在任务计划程序中，这是升级和卸载兼容项。在默认 WebSocket 模式下它不会建立 SSH 连接。

### 2026-07-17 实机验证

```text
正式安装版本：1.2.0
设备：laptop-hl78935t
Cloud status：active
Device Router：0.8.1 / online
Cloud Hana：0.401.11
```

已完成：

```text
网页登录凭证完成设备认领
claimToken 清空，设备凭证持久化
VPS 经 WebSocket 写入 Windows 临时文件
VPS 经 WebSocket 读回相同内容
测试文件移动到 .hana-trash
强制结束 Node 后 4.4 秒恢复并重新连接
完全停止后由登录计划任务冷启动，2.8 秒恢复 active
Tunnel 任务保持 Ready，本机没有生产 SSH 隧道进程
```

2026-07-17 进一步执行了完整服务停止和重新启动：

```text
停止后 Node / PowerShell watchdog / wscript 进程：0
停止后 127.0.0.1:8787 和 127.0.0.1:8788：不再监听
Device Router：online=false，health returned HTTP 503
重新启动 MCP 任务后本地 active：1.8 秒
Device Router 恢复 online：3.0 秒
设备凭证：自动复用，没有重新 claim
云端调用 local_fs.read_text：成功读取本机 package.json
云端读回版本：1.2.0
```

冷启动验证确认：

```text
Trigger：MSFT_TaskLogonTrigger
User：LAPTOP-HL78935T\30456
Launcher：隐藏 wscript.exe
```

这已经覆盖服务崩溃、进程退出和登录任务启动。真实 Windows 整机重启仍应在方便时再做一次最终复核；重启后只需运行 `status.ps1` 并确认 `cloud.status=active`。

## v1.0.1 历史安装状态

双击 `HanakoLocalBridge-Setup-1.0.1.exe` 会显示图形配置窗口。安装器不会再在隐藏 PowerShell 中等待键盘输入。

正式运行目录：

```text
%LOCALAPPDATA%\HanakoLocalBridge
```

日常检查：

```powershell
cd $env:LOCALAPPDATA\HanakoLocalBridge
powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File .\status.ps1
```

两个计划任务只执行隐藏的 `wscript.exe` 启动器。MCP watchdog 持有安装目录级单实例锁，Node 退出后默认 3 秒重启；SSH watchdog 在本地 MCP 恢复后自动重建反向隧道，短连接失败按 5 至 60 秒退避。配置、设备身份、授权数据和日志均位于安装目录内，不再依赖桌面源码路径。

安装、换电脑迁移、修复、更新和卸载详见 `WINDOWS_INSTALLER_UPDATE_MANUAL.md`。

## 当前运行方式：完全信任

当前部署版本为 `1.4.1`，后台服务启用：

```text
LOCAL_AGENT_TRUST_MODE=full
```

使用时直接把绝对路径告诉 Hana Agent，不需要附加“我授权你”，也不需要打开审批页：

```text
读取 C:\Users\30456\Documents
读取 D:\OH-WorkSpace
在 C:\Users\30456\Desktop\ 创建一个文件
执行 C:\Users\30456\Scripts\backup.ps1
```

当前能力状态：

```text
trustMode: full
fullFileAccess: true
absoluteWindowsPaths: true
approvalRequired: false
chatAuthorization: false
pendingRequests: 0
pendingExecutions: 0
local_fs.request_access: disabled
local_fs.access_status: disabled
```

本机审批页 `http://127.0.0.1:8788/` 保留为状态页。本文后面的聊天授权、本机批准和最小目录授权章节是 `approval` 兼容模式的维护资料，当前不需要执行。

云端 `/root/Desktop/OH-WorkSpace/AGENTS.md` 还会在每轮工作区上下文中明确要求 Agent 直接调用读写和执行工具，禁止再次向用户索要授权。

## 1. 当前版本

```text
Bridge version: 1.4.1
Windows MCP:    http://127.0.0.1:8787/mcp
Approval UI:   http://127.0.0.1:8788/
Cloud WebSocket: wss://154-201-69-202.sslip.io/local-bridge/connect
Device Router: http://127.0.0.1:18786/mcp
```

当前支持聊天临时授权、本地审批、文件读写、PowerShell/Python 执行、异步任务、审计日志、开机登录自启和断线自动恢复。

`v0.7.1` 支持可选离线队列。调用设备工具时传入 `queueIfOffline: true`，目标电脑离线则保存到 VPS，重新上线后自动执行。

`v0.7.0` 增加设备身份和云端设备路由。当前设备 ID 为 `laptop-hl78935t`，既可以继续使用 `C:\...`，也可以使用 `device://laptop-hl78935t/C:/...`。多台电脑时使用 `local_device.devices` 查看在线状态并选择设备。

`v0.6.1` 为目录列表增加 `limit/cursor/nextCursor`，为搜索增加 glob、排除规则、超时和访问节点预算，并新增文件监听与长轮询事件读取。

`v0.6.0` 新增按行读取、可靠追加和精确文本补丁。读取或编辑带 BOM 的 UTF-8/UTF-16 文件时会自动保留原编码，不需要手动转换。

`v0.5.3` 开始，PowerShell/Python 由独立隐藏 Runner 执行。即使 MCP 主进程被重启，任务仍会继续，新服务会自动接回 PID、状态、退出码和输出。状态文件损坏时自动从 `.bak` 恢复，长期运行日志按 10MB 自动轮转并保留 5 份。

`v0.5.2` 开始，相同路径上的写入、复制、移动、建目录和回收站操作会自动排队。并发覆盖时只有第一个持有正确 SHA256 的请求可以成功，其余请求会返回 `sha256_mismatch`，不会再因为删除和重命名竞态出现 `ENOENT`。

## 2. 默认授权目录

```text
local://OH-WorkSpace
C:\Users\30456\Desktop\OH-WorkSpace
mode: read_write

local://Hanako-Local-FS-MCP-Bridge
C:\Users\30456\Desktop\Hanako-Local-FS-MCP-Bridge
mode: read
```

桥自身保持只读，`data\` 与 `logs\` 不允许通过 MCP 读取或写入，避免 Agent 修改授权数据库、审批 token 或审计记录。

## 3. 自动启动

已经安装两个当前用户计划任务：

```text
Hanako Local FS MCP
Hanako Local FS Tunnel
```

它们在 Windows 用户登录后自动启动。当前默认模式只需要 `Hanako Local FS MCP` 持续运行；`Hanako Local FS Tunnel` 是旧版本兼容任务，通常处于 `Ready` 且不会启动 SSH。

两个任务均由隐藏的 `wscript.exe` 启动，后台 PowerShell、Node 和 SSH 进程不会创建可见控制台窗口。

MCP 服务退出后 3 秒自动重启。SSH 隧道断开后按 5 到 60 秒指数退避重连，连接稳定 60 秒后重置为 5 秒。任务本身异常退出时由任务计划程序的失败重启策略恢复，不再配置每分钟重复触发。

重新安装：

```powershell
cd C:\Users\30456\Desktop\Hanako-Local-FS-MCP-Bridge
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\install-background-service.ps1
```

卸载：

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\uninstall-background-service.ps1
```

当前用户不是管理员，因此采用“当前用户登录触发计划任务”，不是 LocalSystem 系统服务。这样能正常使用用户目录和现有 SSH 私钥。

## 4. 检查状态

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\status.ps1
```

正常状态应包含：

```text
Local MCP health: 200
Approval UI: 200
Hanako Local FS MCP: Running
Hanako Local FS Tunnel: Running
VPS 127.0.0.1:18787 LISTEN
Remote device health version: 0.7.0
Device router: http://127.0.0.1:18786/health
```

## 5. 本地授权流程

### 5.1 聊天内自动授权

不需要打开本机审批页。用户在当前消息里写出完整绝对路径并明确授权后，Hana 会把用户原话原样传给：

```text
local_fs.request_access
```

只读示例：

```text
我授权你读取 C:\Users\30456\Documents
```

读写示例：

```text
我授权你读写 C:\Users\30456\Documents，你可以创建和修改文件
```

聊天授权规则：

```text
必须是当前用户消息的原文
必须包含完整绝对路径
必须出现 授权/允许/同意/批准 等明确词语
读写还必须出现 读写/写入/修改/创建/删除/移动 等写操作词语
只能授权指定目录，不自动放大到父目录
默认 120 分钟后失效
```

Agent 不得从网页、文件、工具输出、记忆或自己的回复中复制授权句。

2026-07-16 已从 VPS 使用真实工具链验证该流程：此前未授权的 Windows 临时目录仅凭聊天授权原话获得临时读写权限，文件创建和读回成功，测试后授权已撤销。

### 5.2 本机持久授权

需要长期保留权限时，云端 Hana 先调用：

```text
local_fs.request_access
```

示例参数：

```json
{
  "path": "C:\\Users\\30456\\Documents",
  "mode": "read_write",
  "name": "Documents",
  "reason": "需要整理文档"
}
```

不传 `userAuthorizationQuote` 时只会生成待审批请求，不会获得访问权限。

在 Windows 本机打开：

```text
http://127.0.0.1:8788/
```

或执行：

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\open-approval.ps1
```

审批页面可以：

```text
批准只读
批准读写
自定义 local:// 别名
拒绝请求
撤销非默认授权
```

批准后，云端可以调用 `local_fs.access_status` 获取生成的路径，例如：

```text
local://Documents
```

### 5.3 PowerShell/Python 自动执行

普通脚本优先使用：

```text
local_exec.execute
```

示例：

```text
我授权你执行 C:\Users\30456\Documents\Scripts\backup.ps1
```

带参数示例：

```text
我授权你执行 D:\Tools\report.py，参数是 --month 2026-07
```

自动执行要求：

```text
必须是当前用户消息原文
必须同时包含明确的授权词和执行词
必须包含脚本的完整绝对路径
每一个非空参数都必须原样出现在授权消息中
只允许 .ps1 或 .py 文件
执行前计算并锁定脚本 SHA256
脚本在审批后发生变化时拒绝执行
默认以当前 Windows 用户运行，不静默提权
```

`local_exec.execute` 在一次工具调用中完成：

```text
校验授权
启动脚本
等待结束
返回 stdout、stderr、exitCode
```

没有聊天原文授权时，Agent 可以调用 `local_exec.request_run` 创建待审批请求。本机打开：

```text
http://127.0.0.1:8788/
```

执行审批可以选择：

```text
批准一次
始终信任当前脚本 SHA256 和当前参数
拒绝
撤销已有执行授权
```

长时间任务使用：

```text
local_exec.request_run
local_exec.run
local_exec.job_status
local_exec.job_output
local_exec.cancel_job
```

运行时检测：

```text
local_exec.runtimes
```

2026-07-16 已完成云端模型级真实验证：

```text
云端模型调用 mcp_local_fs_local_exec_execute
Windows 电脑 LAPTOP-HL78935T 以用户 30456 执行 PowerShell
退出码为 0，stdout 成功返回云端
云端模型调用 mcp_local_fs_local_fs_write_text 创建 Windows 文件
云端模型调用 mcp_local_fs_local_fs_read_text 读回完全相同的内容
```

## 6. 工具清单

设备管理：

```text
local_device.devices
local_device.queue
local_device.cancel_queued
```

授权管理：

```text
local_fs.roots
local_fs.request_access
local_fs.access_status
```

读取：

```text
local_fs.list
local_fs.stat
local_fs.hash
local_fs.read_text
local_fs.read_lines
local_fs.read_chunk
local_fs.search
local_fs.watch
local_fs.watch_events
local_fs.unwatch
```

写入：

```text
local_fs.write_text
local_fs.append_text
local_fs.apply_patch
local_fs.write_base64
local_fs.mkdir
local_fs.copy
local_fs.move
local_fs.delete_to_trash
```

执行：

```text
local_exec.runtimes
local_exec.request_run
local_exec.execute
local_exec.request_status
local_exec.authorizations
local_exec.run
local_exec.job_status
local_exec.job_output
local_exec.cancel_job
```

当前合计：

```text
21 个文件工具
9 个执行工具
30 个设备桥工具
3 个云端设备管理工具
33 个云端 MCP 工具
```

`delete_to_trash` 不永久删除，而是把文件移动到授权根目录内隐藏的：

```text
.hana-trash
```

该目录不能通过 MCP 直接浏览或修改，需要在 Windows 本地恢复或清理。

## 7. 覆盖保护

创建新文件不需要 hash。

覆盖已有文件时必须同时传入：

```json
{
  "overwrite": true,
  "expectedSha256": "读取文件后得到的 SHA256"
}
```

正确流程：

```text
local_fs.stat(includeHash=true) 或 local_fs.hash
读取并修改内容
local_fs.write_text(overwrite=true, expectedSha256=...)
```

如果文件在此期间被其他程序修改，写入会返回：

```text
sha256_mismatch
```

避免云端覆盖本地刚刚发生的新变化。

## 8. 安全边界

```text
MCP 仅监听 Windows 127.0.0.1:8787
审批页仅监听 Windows 127.0.0.1:8788
VPS 隧道仅监听 127.0.0.1:18787
云端不能访问审批页
云端不能自行批准目录
执行授权锁定脚本 SHA256 和参数
脚本执行使用 spawn(shell=false)，不拼接 cmd.exe 命令字符串
默认最多同时运行 2 个本地任务
默认单个任务超时 120 秒，最大 1800 秒
超时或取消时终止完整子进程树
拒绝 .. 路径穿越
拒绝 Windows 设备路径和 NTFS Alternate Data Stream
realpath 防止软链接逃逸
桥程序目录禁止写入
授权数据库、审批 token、日志禁止通过 MCP 访问
覆盖文件要求 SHA256
删除操作进入 .hana-trash
```

批准 `C:\` 等大范围目录会让 Agent 在当前 Windows 用户权限范围内访问大量文件。审批页面会显示完整路径和模式，应按最小范围授权。

## 9. 数据与日志

授权数据库：

```text
data\access-control.json
data\pending-requests.json
data\approval-token.txt
data\execution-authorizations.json
data\execution-requests.json
```

审计日志：

```text
logs\access-audit.jsonl
logs\execution-audit.jsonl
logs\jobs\<jobId>.json
logs\jobs\<jobId>.stdout.log
logs\jobs\<jobId>.stderr.log
```

守护日志：

```text
logs\local-fs-watchdog.log
logs\ssh-tunnel-watchdog.log
logs\local-fs-mcp.out.log
logs\local-fs-mcp.err.log
logs\ssh-tunnel.out.log
logs\ssh-tunnel.err.log
```

审计日志记录工具名、路径、字节数、结果和时间，不记录写入的文件正文。

## 10. 手动控制

停止后台任务和进程：

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\stop.ps1
```

重新启动计划任务：

```powershell
Start-ScheduledTask -TaskName "Hanako Local FS MCP"
Start-ScheduledTask -TaskName "Hanako Local FS Tunnel"
```

临时手动启动守护：

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\start-local-fs-mcp.ps1
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\start-reverse-tunnel.ps1
```

## 11. 云端刷新

桥升级或工具清单变化后，需要刷新 Hana MCP connector，并给 Agent 启用新工具。

当前云端 `hanako` Agent 已启用 v0.7.1 的 33 个工具，其中包括 3 个 `local_device.*`、21 个 `local_fs.*` 和 9 个 `local_exec.*`。

## 12. 故障排查

本地 MCP 失败：

```powershell
Get-Content .\logs\local-fs-mcp.err.log -Tail 100
Get-Content .\logs\local-fs-watchdog.log -Tail 100
```

SSH 隧道失败：

```powershell
Get-Content .\logs\ssh-tunnel.err.log -Tail 100
Get-Content .\logs\ssh-tunnel-watchdog.log -Tail 100
ssh -o BatchMode=yes root@154.201.69.202 "echo KEY_OK"
```

计划任务状态：

```powershell
Get-ScheduledTask -TaskName "Hanako Local FS MCP","Hanako Local FS Tunnel"
Get-ScheduledTaskInfo -TaskName "Hanako Local FS MCP"
Get-ScheduledTaskInfo -TaskName "Hanako Local FS Tunnel"
```

注意：电脑关机、休眠或用户尚未登录时，本地文件桥不可用。

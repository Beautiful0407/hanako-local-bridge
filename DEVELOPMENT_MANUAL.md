# Hanako 本地文件与执行桥 MCP 开发维护手册

## Rust 2.0 Alpha 开发入口

Rust 工作区位于：

```text
Cargo.toml
crates/hanako-bridge-core
apps/hanako-bridge
apps/hanako-device-router
apps/hanako-installer
apps/hanako-manager
apps/hanako-updater
tests/rust-integration.test.cjs
tests/rust-device-router.test.cjs
tests/rust-audit.test.cjs
tests/rust-recovery.test.cjs
tests/rust-installer-smoke.ps1
tests/rust-update-smoke.ps1
```

`hanako-bridge-core` 负责兼容配置、设备身份、路径解析、更新清单和原子 JSON 存储；`hanako-bridge` 负责 MCP、文件操作、脚本执行、云端连接、审批、服务控制和管理 API；`hanako-manager` 使用 Winit、Wry、WebView2 和系统托盘承载管理界面；`hanako-maintenance` 负责签名下载、事务更新和回滚；`hanako-bootstrap` 负责内嵌安装、快捷方式和卸载；`hanako-device-router` 负责 Linux 多设备路由和离线队列。

当前 Windows Rust 版本为 `2.0.0-alpha.9`，云端 Rust 路由器为兼容的 `2.0.0-alpha.2`。Alpha 8 将 Release `hanako-bridge.exe` 编译为 Windows GUI 子系统，计划任务启动时不会再出现控制台窗口；Alpha 9 在登录触发之外增加每分钟周期触发，并继续使用 `IgnoreNew` 保证正常运行时只有一个 Bridge。这样既能处理正常非零退出，也能覆盖外部强制终止返回 `0xFFFFFFFF`、`RestartOnFailure` 未触发的情况。Debug 构建仍保留控制台，服务命令的重定向 JSON 输出也保持可用。关闭管理器只退出管理界面，不会终止独立的 MCP 后台任务；Windows 稳定渠道仍为 `1.4.9`。

完整质量门：

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --workspace --release
cargo build --workspace
node tests\rust-integration.test.cjs
node tests\rust-audit.test.cjs
node tests\rust-recovery.test.cjs
node tests\rust-device-router.test.cjs
powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File tests\rust-update-smoke.ps1
powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File tests\rust-installer-smoke.ps1
```

发布时先用 `hanako-maintenance pack` 生成签名 ZIP 和 manifest，再设置 `HANA_INSTALLER_PAYLOAD` 构建 `hanako-bootstrap`。生产私钥只位于 `%USERPROFILE%\.hanako-update-signing\private-key.xml`，不得提交到 Git。

Rust Alpha 的完整构建、测试、数据兼容和迁移边界见 `RUST_MIGRATION.md`。

## v1.4.1 安全入口、WSS 与签名更新

`server.cjs` 的 `/mcp` 只接受 loopback Host、拒绝浏览器 Origin，并要求 `Authorization: Bearer <data\approval-token.txt>`。MCP 和审批接口请求体上限为 1 MiB。

官方云端地址为 `wss://154-201-69-202.sslip.io/local-bridge/connect`。Nginx 使用公开证书代理到 `127.0.0.1:14500`，Certbot 定时任务负责续期。

远程更新清单使用 `update-signature.ps1` 生成和验证 RSA-SHA256 签名。生产私钥只保存在 `%USERPROFILE%\.hanako-update-signing\private-key.xml`；仓库、ZIP 和安装目录只包含 `update-public-key.xml`。稳定版更新清单和 ZIP 由 `https://154-201-69-202.sslip.io/local-bridge/releases/` 提供，Alpha 清单和 ZIP 位于其 `alpha/` 子目录；源码仓库保持私有也不会影响客户端更新。

配置保存只替换第一个读写根目录，并保留其他根目录。WinUI 管理器窗口尺寸限制在当前 Windows 工作区以内。

## v1.3.1 管理命令 JSON 边界修复

WinUI 通过 `manager-command.ps1` 启动 PowerShell 子进程，并要求标准输出只包含一条压缩 JSON。`repair.ps1`、`stop.ps1` 和后台任务安装脚本会使用 `Write-Host` 输出 `Stopped ...`、`Installed background tasks ...` 等文字；这些内容进入标准输出后会导致 `System.Text.Json` 在第一个字母处报 `invalid start of a value`。

修复策略：

```text
manager-command.ps1
  -> 屏蔽 Information / Warning / Verbose / Debug 流
  -> 标准输出只写最终 JSON

manager-core.ps1
  -> stop / restart / repair 调用时屏蔽辅助输出流
  -> 保留错误流，真实失败仍返回非零退出码

BridgeCommandService.cs
  -> 优先解析全部 stdout
  -> 异常时从末尾查找最后一条完整 JSON

bridge-common.ps1
  -> 普通 repair 不结束 HanakoBridgeManager.exe
  -> 只结束已知 watchdog、server.cjs 和旧隧道进程
  -> 不结束仅在命令行中引用安装目录的其他 PowerShell 或工具
  -> installer / update 仍使用专门逻辑关闭管理器后替换文件
```

回归测试：

```powershell
npm.cmd run test:manager-command
```

`tests/manager-command.test.ps1` 使用临时 `manager-core.ps1` 主动输出 `Stopped HanakoBridgeManager.exe ...`，并断言命令层最终只产生一行可解析 JSON。

## v1.3.0 WinUI 3 Windows 图形管理器

管理器使用“原生界面 + JSON 命令层 + PowerShell 核心”三层结构：

```text
manager-winui\
  -> .NET 10 / WinUI 3 / Windows App SDK 2.3.1
  -> 概览、诊断与修复、云端设备、日志、深浅色主题
  -> 5 秒自动刷新

manager-command.ps1
  -> 把 snapshot / action / cloud-query / logs / log-tail 转成 JSON
  -> 只通过子进程环境变量接收网页登录密钥

manager-core.ps1
  -> 读取运行时配置
  -> 检查计划任务、隐藏启动器和服务进程
  -> 请求本地 health / client-identity
  -> 汇总 active / pending_claim / offline
  -> 执行 start / stop / restart / repair
  -> 临时登录 Hana 并查询或认领设备

manager-ui.ps1
  -> 旧 WinForms 回退界面
```

启动链：

```text
开始菜单快捷方式
  -> wscript.exe //B run-manager.vbs
    -> manager\HanakoBridgeManager.exe --smoke-test
      -> 成功：启动 WinUI 3 管理器
      -> 失败：隐藏启动 manager-ui.ps1
```

WinUI 项目使用：

```text
TargetFramework: net10.0-windows10.0.22621.0
TargetPlatformMinVersion: Windows 10 2004
RuntimeIdentifier: win-x64
WindowsPackageType: None
SelfContained: true
WindowsAppSDKSelfContained: true
```

`build-manager-winui.ps1` 自动优先选择 `%USERPROFILE%\.dotnet10\dotnet.exe`，执行 restore 和 publish，并验证：

```text
HanakoBridgeManager.exe
App.xbf
MainWindow.xbf
HanakoBridgeManager.pri
```

Windows App SDK 发布阶段默认可能漏复制应用 XBF/PRI；`HanakoBridgeManager.csproj` 的 `CopyWinUIRuntimeResourcesToPublish` target 会把这三项补入发布目录。发布后必须执行 `--smoke-test`，不能只检查 EXE 是否存在。

安全边界：

```text
cloud-identity.json 只在本机读取
manager snapshot 只暴露 credentialPresent / claimTokenPresent
诊断报告不包含 credential、claimToken、privateKey
网页登录密钥只保存在 WinUI PasswordBox 和子进程环境变量内
请求结束后清空 PasswordBox；密钥不进入命令行、配置、日志或诊断报告
```

日志读取兼容历史文件中混合存在的 UTF-8 与无 BOM UTF-16LE 片段。`ConvertFrom-HanakoMixedLogBytes` 按行判断编码，统一换行并移除尾部 NUL，避免 WinUI 日志框出现空白或乱码。

测试入口：

```powershell
npm.cmd run test:manager
npm.cmd run build:manager
npm.cmd test
```

`tests/manager-core.test.ps1` 使用临时目录、随机端口和不存在的计划任务，验证 URL 规范化、离线诊断、固定数组 JSON 结构、凭证存在状态、混合日志编码和秘密不泄露。`tests/installer-smoke.ps1` 还会启动安装后的 WinUI 管理器，并验证运行中的管理器不会阻塞覆盖升级。

## v1.2.0 主动云端连接

`v1.2.0` 的默认传输从 SSH 反向隧道切换为 WebSocket：

```text
lib/cloud-connector.cjs
  -> 保存 Ed25519 设备身份
  -> 主动连接 cloud.url
  -> 发送签名设备证明
  -> 等待浏览器 claim
  -> 保存云端设备凭证
  -> 转发 MCP JSON-RPC
```

运行时入口位于 `server.cjs`。本地 HTTP MCP 和 WebSocket 共用同一个 `handleRpc()`，因此文件、图片、脚本、审计和错误语义不会因为传输方式改变。

云端对应实现：

```text
server/local-bridge-gateway.ts
server/routes/local-bridge.ts
desktop/src/react/services/local-bridge-device.ts
```

Device Router `0.8.1` 的 `registerDevice()` 支持显式 `url`、`healthUrl` 和私有 `mcpToken`。WebSocket 设备注册为 Hana 进程的内部回环转发地址；旧 SSH 设备会转发 Bearer Token，但公开设备列表不会显示它。

持久文件：

```text
data\device.json
data\cloud-identity.json
data\access-control.json
data\execution-*.json
logs\
```

`cloud-identity.json` 包含私钥、认领令牌或设备凭证，安装和升级时必须保留，不能加入公开发布包或日志。

标准验证：

```powershell
npm.cmd test
npm.cmd run build:installer
npm.cmd run test:installer
```

云端补充验证：

```text
LocalBridgeGateway 单元测试
local-bridge 路由安全测试
local-bridge-device 浏览器认领测试
TypeScript typecheck
```

完整协议与时序见 `CLOUD_WEBSOCKET_ARCHITECTURE.md`。

## v1.0.0 Windows 安装与运行时配置

`v1.0.0` 新增四个维护边界：

```text
代码和自带运行时：安装包更新时可替换
config.json：升级时保留
data\：升级时保留
logs\：升级时保留
```

Node 服务通过 `lib/runtime-config.cjs` 原生读取配置；PowerShell 服务脚本通过 `bridge-common.ps1` 和 `scripts/runtime-config-cli.cjs` 读取同一份规范化结果。环境变量仍具有最高优先级，用于测试和临时覆盖。

Windows 生命周期文件：

```text
configure.ps1
install-background-service.ps1
repair.ps1
status.ps1
stop.ps1
uninstall-background-service.ps1
update.ps1
build-installer.ps1
installer\bootstrap-install.ps1
tests\installer-smoke.ps1
```

稳定性策略：

```text
计划任务动作只启动 wscript.exe
wscript 以窗口样式 0 启动 PowerShell
MCP 和 Tunnel 各自持有安装目录级命名 Mutex
Node 退出后 watchdog 循环重启
Tunnel 先验证本地 MCP，再检查/重建远端监听
所有状态、停止和进程识别均按安装目录与 config.json 解析
```

构建和验证：

```powershell
npm.cmd test
npm.cmd run build:installer
npm.cmd run test:installer
```

安装器烟雾测试使用临时安装目录、随机端口和独立计划任务名，真实验证 EXE 安装、隐藏启动、强杀 Node 后恢复、本地清单更新、持久数据保留和卸载清理，不影响生产任务。

## v0.7.1 离线队列实现说明

路由后的设备工具 schema 都增加：

```text
queueIfOffline: boolean
```

当该值为 `true` 且目标设备离线时，路由器把调用保存到：

```text
/opt/hanako-local-device-router/offline-queue.json
```

设备健康检查恢复为在线后自动重放。队列项状态：

```text
queued
running
completed
failed
cancelled
```

管理工具：

```text
local_device.queue
local_device.cancel_queued
```

## v0.7.0 多电脑设备身份与云端路由实现说明

每台 Windows 设备启动时会创建或更新：

```text
data\device.json
```

字段包括 `id/name/hostname/platform`。默认 ID 来自计算机名，也可以通过：

```text
LOCAL_AGENT_DEVICE_ID
LOCAL_AGENT_DEVICE_NAME
```

覆盖。

文件和脚本路径新增：

```text
device://<deviceId>/C:/Users/name/file.txt
```

直接绝对路径和 `local://` 仍保持兼容。路径中的设备 ID 与当前桥不一致时返回 `wrong_device`。

VPS 设备路由器文件：

```text
cloud/device-router.cjs
cloud/devices.example.json
cloud/hanako-local-device-router.service
```

路由器监听 `127.0.0.1:18786`，按 `deviceId` 或 `device://` 选择设备，缓存 30 个设备工具，并增加 `local_device.devices`。设备离线时返回 `device_offline`，不同设备出现在同一次调用时返回 `cross_device_operation_not_supported`。

每台电脑通过不同的 `HANA_TUNNEL_REMOTE_PORT` 暴露到 VPS 回环地址。当前笔记本使用 `18787`。

## v0.6.1 大目录分页、预算搜索与文件监听实现说明

`local_fs.list` 新增：

```text
limit: 默认 200，最大 1000
cursor: 上一页返回的 opaque nextCursor
totalEntries / offset / nextCursor
```

列表先按“目录在前、名称排序”处理，再只对当前页条目读取 metadata，避免大目录一次 stat 全部文件。

`local_fs.search` 新增：

```text
glob: **/*.txt 等模式
exclude: 相对路径排除规则
timeoutMs: 100 到 30000
maxVisited: 1 到 100000
visited / visitedDirectories / skippedLinks
truncationReasons
```

搜索使用迭代栈和真实目录集合，跳过 symlink/junction 并防止目录循环。

文件监听工具：

```text
local_fs.watch
local_fs.watch_events
local_fs.unwatch
```

监听状态保存在 MCP 内存中，使用递增 sequence、最多 1000 条事件环、去抖和最长 30 秒长轮询。MCP 重启后调用方需要重新创建 watch。

## v0.6.0 文件编辑工具与编码保真实现说明

新增工具：

```text
local_fs.read_lines
local_fs.append_text
local_fs.apply_patch
```

文本解码支持：

```text
UTF-8
UTF-8 BOM
UTF-16LE BOM
UTF-16BE BOM
```

`append_text` 在路径锁内重新读取当前内容并原子替换，因此并发追加不会互相覆盖。调用方可以额外传入 `expectedSha256` 做乐观并发检查。

`apply_patch` 必须提供 `expectedSha256`，每个 edit 还要提供精确 `oldText/newText`。默认要求旧文本只出现一次，也可以通过 `expectedOccurrences` 声明准确数量；数量不一致返回 `patch_context_mismatch`。

## v0.5.3 可恢复执行任务与状态自愈实现说明

执行任务现在分为两层：

```text
MCP ExecutionController
  -> 写入 job spec 和 starting summary
  -> 启动隐藏 detached Node Runner
  -> 持久化 runner PID 并监控 result 文件

Node Runner
  -> 启动 PowerShell/Python
  -> stdout/stderr 直接写入 job 日志
  -> 使用 close 事件确认输出句柄已经关闭
  -> 原子写入 result JSON
```

MCP 启动时扫描状态为 `starting/running` 的 job summary。Runner 仍存活时重新监控；Runner 已写结果时立即恢复准确状态；Runner 和结果都不存在时将任务标记为失败并留下明确原因。

JSON 状态文件写入前保留 `.bak`。主文件无法解析时重命名为 `.corrupt-<timestamp>-<random>`，然后恢复最近备份；主文件和备份都不可用时才回退到空状态。

日志策略：

```text
access-audit.jsonl / execution-audit.jsonl: 10MB，5 份轮转
MCP / SSH / watchdog 日志: 10MB，5 份轮转
单个完成任务 stdout/stderr: 默认各保留最后 1MB
```

## v0.5.2 并发写入与可靠覆盖实现说明

`lib/tools.cjs` 使用规范化绝对路径作为 keyed mutex 键。相同目标上的创建、覆盖、复制、移动、建目录和回收站操作会串行执行，不同路径仍可并行。

覆盖已有文件的提交顺序：

```text
第一次验证 expectedSha256
写入同目录临时文件
提交前第二次验证 expectedSha256
把旧文件重命名为同目录备份
把临时文件重命名为正式文件
提交失败时恢复旧文件
提交成功后清理备份
```

并发测试会用同一个旧 SHA256 同时提交 8 次覆盖，要求严格只有 1 次成功，其余请求全部返回 `sha256_mismatch`。

## v0.5.1 完全信任与无窗口后台模式实现说明

当前生产配置由 `run-local-fs-service.ps1` 注入：

```text
LOCAL_AGENT_TRUST_MODE=full
LOCAL_FS_MCP_ALLOW_CHAT_AUTHORIZATION=0
```

`AccessController` 在 full 模式下：

```text
识别绝对 Windows 盘符路径
为每个盘符生成 Drive-C、Drive-D 等隐式 read_write grant
requestAccess 立即返回 authorized
跳过 bridge 程序、data 和 logs 的 MCP 访问限制
启动时把历史 pending 请求标记为 bypassed_full_trust
仍拒绝 NUL、设备路径、ADS 和 .. 路径穿越
```

`ExecutionController` 在 full 模式下：

```text
requestRun 自动生成 source=full_trust 的一次性 authorization
不读取或验证 userAuthorizationQuote
不创建本机待审批请求
仍锁定 runtime、scriptPath、SHA256、arguments、cwd 和 timeout
仍使用 spawn(shell=false)，并保留作业与审计日志
```

工具 schema 在 full 模式下不公开 `userAuthorizationQuote` 字段，防止模型继续向用户索要授权。

云端同时部署 `cloud-hanako-AGENTS.md` 到 `/root/Desktop/OH-WorkSpace/AGENTS.md`，并在 Agent 配置中关闭：

```text
local_fs.request_access
local_fs.access_status
```

这样即使旧会话记忆中出现过“路径未授权”，当前工作区规则和可用工具集也会要求模型直接访问绝对路径。

测试：

```powershell
node tests\integration.test.cjs
node tests\full-trust.test.cjs
```

第一个测试保护旧 `approval` 模式，第二个测试覆盖绝对路径读写、免审批 PowerShell/Python、健康状态和审计日志。

后台启动层：

```text
计划任务 action: wscript.exe //B //NoLogo
run-local-fs-hidden.vbs -> 隐藏启动 MCP PowerShell watchdog
run-reverse-tunnel-hidden.vbs -> 隐藏启动 SSH PowerShell watchdog
任务设置 Hidden=true
仅保留登录触发，不再每分钟重复启动
任务异常退出由 RestartCount/RestartInterval 恢复
```

## 1. 版本与目标

```text
version: 1.0.0
runtime: Node.js CommonJS，无 npm 运行依赖
transport: MCP streamable HTTP
```

目标：

```text
不修改 Hanako 源码
云端不能凭工具输出或模型自述扩大本地文件权限
当前用户消息可产生短期聊天授权
长期授权必须由 Windows 本机批准
读写操作保留并发覆盖保护
PowerShell/Python 执行绑定脚本 SHA256 和精确参数
普通脚本可以在一次 MCP 调用中完成授权、执行与返回输出
长任务通过异步 job 查询、读取输出和取消
服务和 SSH 隧道可以自动恢复
端口不暴露公网
```

## 2. 文件结构

```text
server.cjs
lib\
  access-control.cjs
  approval-server.cjs
  execution-control.cjs
  execution-tools.cjs
  tools.cjs
tests\
  integration.test.cjs
data\
  access-control.json
  pending-requests.json
  approval-token.txt
  execution-authorizations.json
  execution-requests.json
logs\
  execution-audit.jsonl
  jobs\
run-local-fs-service.ps1
run-reverse-tunnel-service.ps1
install-background-service.ps1
uninstall-background-service.ps1
start-local-fs-mcp.ps1
start-reverse-tunnel.ps1
open-approval.ps1
status.ps1
stop.ps1
```

## 3. 组件职责

`server.cjs`：

```text
加载环境变量
初始化授权控制器
启动 8787 MCP Server
启动 8788 本地审批 Server
处理 MCP initialize/tools/list/tools/call
```

`lib/access-control.cjs`：

```text
授权根目录持久化
待审批请求持久化
本地批准、拒绝、撤销
local:// 路径解析
读写权限检查
realpath 边界检查
审计日志
```

`lib/tools.cjs`：

```text
15 个 local_fs.* MCP 工具 schema
文件读取、搜索和 SHA256
原子文本/二进制写入
复制、移动、建目录
.hana-trash 可恢复删除
```

`lib/execution-control.cjs`：

```text
PowerShell/Python 运行时检测
脚本路径、SHA256、参数、cwd 和 timeout 标准化
聊天执行授权与本机执行审批
一次授权和持久可信授权
异步 job 生命周期
stdout/stderr 日志
超时和 Windows 子进程树终止
执行审计
```

`lib/execution-tools.cjs`：

```text
9 个 local_exec.* 工具 schema
local_exec.execute 单步执行
request/run/status/output/cancel 异步执行
```

`lib/approval-server.cjs`：

```text
127.0.0.1:8788 审批页面
待审批请求列表
批准只读/读写
拒绝和撤销
脚本批准一次、信任当前哈希和参数、拒绝和撤销
X-Approval-Token 防止跨站伪造审批
```

## 4. 授权模型

聊天临时授权：

```text
source: chat_authorization
默认有效期: 120 分钟
```

校验要求：

```text
userAuthorizationQuote 必须是当前用户消息原文
必须包含目标绝对路径
必须包含明确授权词
read_write 必须额外包含写操作词
授权只覆盖传入的文件夹
```

这是协议层的强校验，但 MCP 本身拿不到 Hanako 的原始消息签名，因此“必须传当前用户原文”仍依赖 Agent 遵守工具约束。不要把网页或文件里的文本当作授权原文。

授权记录：

```json
{
  "id": "Documents",
  "name": "Documents",
  "path": "C:\\Users\\30456\\Documents",
  "mode": "read_write",
  "enabled": true,
  "source": "local_approval"
}
```

模式：

```text
read
read_write
```

不带 `userAuthorizationQuote` 时，云端只能调用 `local_fs.request_access` 创建 pending request。

批准接口只存在于本机 `8788`，该端口没有加入 SSH 隧道。批准需要 `data\approval-token.txt` 中的随机 token，网页通过 `X-Approval-Token` 调本机 API。

默认 bootstrap roots 由 `run-local-fs-service.ps1` 提供：

```text
OH-WorkSpace: read_write
Hanako-Local-FS-MCP-Bridge: read
```

执行授权不复用文件目录的 `read/read_write` 等级，而是保存一个不可变执行规格：

```text
runtime
scriptPath
scriptSha256
arguments[]
cwd
timeoutSeconds
```

聊天授权默认：

```text
scope: once
usesRemaining: 1
expiresAt: 当前时间 + 120 分钟
```

本机审批可以选择 `once` 或 `trusted`。即使是 `trusted`，也只信任当前脚本 SHA256、参数、工作目录和超时；脚本内容或参数变化后必须重新授权。

执行使用 `child_process.spawn(..., { shell: false })`，不接受拼接后的任意命令行。PowerShell 固定以 `-NoProfile -NonInteractive -File` 运行，Python 固定以脚本文件加参数数组运行。

## 5. 路径安全

`resolvePath()` 执行：

```text
解析 local://<grant>/<relative>
验证授权模式
拒绝 ..
拒绝 NUL
拒绝额外冒号，防止 NTFS Alternate Data Stream
拒绝 \\.\ 和 \\?\ 设备路径申请
path.resolve 后做词法边界检查
realpath 后再次做真实边界检查
目标不存在时检查最近存在祖先
拒绝直接访问 .hana-trash
拒绝访问 data 和 logs
拒绝写入桥程序目录
```

新的访问申请只接受：

```text
本地盘符绝对目录，例如 C:\Users\name\Documents
```

当前不接受 UNC、网络共享和 Windows 设备路径。

## 6. 写入语义

`write_text` 和 `write_base64`：

```text
新文件可以直接创建
覆盖默认拒绝
覆盖必须 overwrite=true
覆盖必须 expectedSha256
写入先生成同目录临时文件，再 rename
默认单次最大写入 4MB
```

环境变量：

```text
LOCAL_FS_MCP_MAX_WRITE_BYTES
```

`copy`：

```text
source 需要 read
destination 需要 read_write
destination 必须不存在
```

`move`：

```text
source 和 destination 都需要 read_write
跨卷时自动执行 copy + remove
destination 必须不存在
```

`delete_to_trash`：

```text
不永久删除
移动到授权 root\.hana-trash
根目录本身不可删除
.hana-trash 不出现在 list/search 中
```

## 7. MCP 工具

```text
local_fs.roots
local_fs.request_access
local_fs.access_status
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
local_fs.write_text
local_fs.append_text
local_fs.apply_patch
local_fs.write_base64
local_fs.mkdir
local_fs.copy
local_fs.move
local_fs.delete_to_trash
```

执行工具：

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

`local_exec.execute` 是普通任务的首选入口。它内部依次调用：

```text
requestRun()
runAuthorization()
waitForJob()
readJobOutput()
```

异步接口用于长任务或需要主动取消的任务。

新增工具后：

```text
更新 createToolDefinitions()
更新 createToolRunner()
增加 integration.test.cjs 覆盖
本地 tools/list 验证
VPS tools/list 验证
调用 Hanako refresh-tools
更新 Agent 工具白名单
```

## 8. 审批 HTTP API

```text
GET  /                     审批页面
GET  /health               本机健康检查
GET  /api/state            授权和请求状态
POST /api/requests/:id/approve
POST /api/requests/:id/deny
POST /api/grants/:id/revoke
POST /api/execution/requests/:id/approve
POST /api/execution/requests/:id/deny
POST /api/execution/authorizations/:id/revoke
```

除首页和 health 外，API 需要：

```text
X-Approval-Token
```

审批服务必须固定绑定：

```text
127.0.0.1
```

不要将 8788 加入 SSH 隧道。

## 9. 后台守护

计划任务：

```text
Hanako Local FS MCP
Hanako Local FS Tunnel
```

任务使用当前用户的 Interactive logon token，在用户登录后运行。

`run-local-fs-service.ps1`：

```text
node server.cjs 前台运行
退出后等待 3 秒
无限循环重启
```

`run-reverse-tunnel-service.ps1`：

```text
ssh -N -R 前台运行
ServerAliveInterval=30
ServerAliveCountMax=3
短连接失败后按 5 到 60 秒指数退避
连接稳定 60 秒后恢复为 5 秒
无限循环重连
```

任务计划设置：

```text
登录触发
StartWhenAvailable
RestartCount=999
RestartInterval=1 minute
ExecutionTimeLimit=0
MultipleInstances=IgnoreNew
```

当前任务还配置了：

```text
Hidden=true
仅登录触发
任务被外部终止后由失败重启策略重新拉起
```

SSH 陈旧监听通过 `ss` 精确解析监听 PID，再终止对应 `sshd`：

```text
ss -ltnp -> pid=<number> -> kill <pid>
```

避免旧的 PID 解析命令在没有 PID 时调用 `kill` 产生错误。

## 10. 测试

运行：

```powershell
node .\tests\integration.test.cjs
```

覆盖：

```text
health 和 tools/list
创建文本文件
无 hash 覆盖拒绝
正确 hash 覆盖
mkdir/copy/move
桥程序目录写入拒绝
.. 路径穿越拒绝
目录申请和本地批准
批准后新 root 写入
delete_to_trash
PowerShell/Python 运行时检测
聊天单次执行授权
本机执行审批
local_exec.execute 单步执行和输出
异步 run/status/output
脚本 SHA256 变化后拒绝
执行审计日志
审计日志
```

每次改动还应执行：

```powershell
node --check .\server.cjs
node --check .\lib\access-control.cjs
node --check .\lib\approval-server.cjs
node --check .\lib\execution-control.cjs
node --check .\lib\execution-tools.cjs
node --check .\lib\tools.cjs
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\status.ps1
```

## 11. 当前部署验证

2026-07-16 已验证：

```text
计划任务均为 Running
Windows 8787 health 返回 v0.7.0，device.id=laptop-hl78935t
Windows 8788 approval health 返回 200
VPS 127.0.0.1:18787 可以访问设备桥 v0.7.0
VPS 127.0.0.1:18786 可以访问设备路由器 v0.7.0
云端写入 OH-WorkSpace 测试文件成功
云端读取内容和 SHA256 成功
测试文件已清理
主动杀死 node 后 3 秒内由本地 watchdog 拉起新进程
主动杀死 ssh 后由隧道 watchdog 清理远端残留并自动重连
全部 wscript、PowerShell、Node 和 SSH 后台进程 MainWindowHandle=0
Hanako connector 已刷新 33 个工具
hanako Agent 已启用全部 33 个工具
聊天授权缺少完整路径时拒绝
聊天明确授权读写后自动生成 120 分钟临时 grant
VPS 使用真实聊天授权原话自动授权未授权 Windows 临时目录
VPS 在临时授权目录创建并读回文件成功
测试后已撤销临时 grant 并清理目录
云端模型调用 mcp_local_fs_local_exec_runtimes 成功检测本机运行时
云端模型调用 mcp_local_fs_local_exec_execute 成功执行 PowerShell
执行发生在 LAPTOP-HL78935T 的用户 30456 下
PowerShell exitCode=0，stdout 成功返回云端
```

## 12. 回滚

停止并卸载后台任务：

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\uninstall-background-service.ps1
```

授权数据库可以在停止服务后备份或删除：

```text
data\access-control.json
data\pending-requests.json
data\approval-token.txt
```

删除授权数据库会重置动态授权；下次启动仍会恢复脚本中定义的 bootstrap roots。

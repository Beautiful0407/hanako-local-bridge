# Hanako Local Agent MCP Bridge

云端 Hana Agent 使用的 Windows 本地文件读写与 PowerShell/Python 执行桥。

## Rust 2.0 Alpha 迁移

仓库当前包含 `2.0.0-alpha.12` Rust 实现，用于替换 Node.js、PowerShell watchdog 和自包含 WinUI/.NET 管理器。Rust 版本已经覆盖本地 MCP 服务、31 个文件与执行工具、云端 WebSocket、后台任务、WebView2 托盘管理器、签名更新器、内嵌安装器和 Linux 多设备路由器。

`2.0.0-alpha.12` 修复公网更新包下载中途断线会直接失败的问题：维护器保留部分 ZIP，通过 HTTP Range 从断点续传；服务器忽略 Range 时安全重头下载，最终仍必须通过签名清单中的大小和 SHA256 校验。Alpha 11 的快捷方式/卸载信息迁移和 Alpha 10 的统一入口继续保留。Alpha 12 已覆盖真实远程包探针、断线续传、安装、覆盖、单实例、自愈、任务恢复、审计和云端协议测试。云服务器的设备路由器仍运行兼容的 Rust Alpha 2，Windows 正式渠道仍为稳定版 `1.4.9`。

从现在起，Rust 工作区中的 Bridge、Manager、Maintenance、Bootstrap 和 Device Router 统一属于一个 Hanako Local Bridge 产品。它们可以因为后台常驻、自更新替换和 Windows/Linux 运行环境而使用多个内部进程或构建目标，但不分别面向用户安装、配置、升级或维护。

## 当前稳定配置：v1.4.9 安全连接与 HTTPS 签名更新版

`v1.4.1` 为本地 MCP 增加 Bearer Token、Origin/Host 防护和请求体上限；云端改用受信任的 HTTPS/WSS；设置保存会保留额外根目录；远程更新必须通过 RSA 签名、SHA256 和大小校验，并从云服务器的公开 HTTPS 稳定清单获取。GitHub 源码仓库可以继续保持私有。

`v1.3.1` 修复了点击“检测并修复”后，PowerShell 状态文字污染 JSON 导致 WinUI 显示解析错误的问题；修复过程中管理器窗口也不会再被后台进程清理逻辑结束。

`v1.3.0` 把本地管理器迁移到 WinUI 3，界面使用现代 Windows Fluent 控件；后台服务、配置和设备数据仍沿用原有 PowerShell/Node 实现。安装后可从开始菜单打开：

```text
开始菜单
  -> Hanako Local Bridge
    -> Hanako Local Bridge Manager
```

管理器提供：

```text
概览：设备 ID、版本、端口、任务、进程和云端状态
诊断与修复：检测、启动、停止、重启、一键修复、复制安全诊断报告
云端设备：查询所有电脑、登录并认领本机
日志：查看 watchdog、MCP 和隧道日志
```

原生管理器启动前会先执行快速自检。原生程序缺失或自检失败时，`run-manager.vbs` 自动回退到旧 WinForms 管理器，不影响修复和设备认领。

如果 Hana 网页只显示一台电脑，先在缺失的电脑安装 `v1.4.9`，打开管理器检查云端状态：

```text
active：已经认领并在线
pending_claim：已连接，输入 Hana 网页访问密钥并点击“登录并认领本机”
offline：未连接，点击“检测并修复”
```

访问密钥只用于当次登录和查询，不会写入 `config.json`、日志或诊断报告。

`v1.2.0` 改为由 Windows 本地桥主动连接云端 Hana，不再要求每台电脑生成 SSH 密钥、上传公钥或分配反向隧道端口。

首次使用流程：

```text
安装本地桥
本地桥主动连接 wss://154-201-69-202.sslip.io/local-bridge/connect
浏览器打开 https://154-201-69-202.sslip.io/desktop/
输入 Hana 网页访问密钥
网页自动发现并认领当前电脑
本地桥保存独立设备凭证
以后开机和断线后自动恢复
```

网页登录密钥只发送给 Hana 登录接口，不会发送给本地桥。本地桥使用自己的 Ed25519 私钥、一次性 `claimToken` 和云端签发的设备凭证。

当前版本组合：

```text
Windows Stable Bridge: 1.4.9
Windows Rust Preview:  2.0.0-alpha.14
Cloud Hana:            current deployed build
Device Router:         2.0.0-alpha.2 (Rust)
```

2026-07-17 实机验证已通过自动认领、WebSocket 文件写入/读回、Node 崩溃恢复和登录任务冷启动。真实整机重启复核待用户方便时执行。

完整架构、安全边界和认领流程见 `CLOUD_WEBSOCKET_ARCHITECTURE.md`。

## 历史安装说明

`v1.0.1` 修复双击安装器后没有可见反馈的问题。普通双击现在显示 Windows 图形配置窗口以及成功、取消或错误提示；`/Q` 参数继续用于静默安装。

`v1.0.0` 把本地桥从桌面源码目录升级为可安装、可修复、可更新的 Windows 后台程序：

```text
默认安装目录：%LOCALAPPDATA%\HanakoLocalBridge
配置文件：    %LOCALAPPDATA%\HanakoLocalBridge\config.json
持久数据：    %LOCALAPPDATA%\HanakoLocalBridge\data
运行日志：    %LOCALAPPDATA%\HanakoLocalBridge\logs
```

安装包自带 Node.js。登录 Windows 后，隐藏计划任务通过 `wscript.exe` 启动单实例 watchdog；MCP 进程异常退出后自动重启，WebSocket 断线后按 3 至 60 秒指数退避重连。日常运行不会弹出 PowerShell 窗口。

发布文件：

```text
release\HanakoLocalBridge-Setup-1.4.1.exe
release\HanakoLocalBridge-1.4.1-win-x64.zip
release\update-manifest.json
```

完整安装、迁移、更新和修复流程见 `WINDOWS_INSTALLER_UPDATE_MANUAL.md`。

## v0.7.1 多电脑离线队列

当前 Windows 后台服务设置了：

```text
LOCAL_AGENT_TRUST_MODE=full
```

实际行为：

```text
直接接受 C:\... 和 D:\... 绝对路径
在当前 Windows 用户 30456 的系统权限范围内读写全部可访问文件
不再调用 request_access 获取许可
不再要求 userAuthorizationQuote
不再产生本机文件或脚本待审批请求
PowerShell/Python 脚本可以直接执行
云端 hanako 已停用 request_access 和 access_status
```

示例：

```text
读取 C:\Users\30456\Documents
列出 D:\OH-WorkSpace
把文件写入 C:\Users\30456\Desktop\result.txt
执行 C:\Users\30456\Scripts\report.ps1
```

`http://127.0.0.1:8788/` 现在仅用于查看状态。下面关于逐目录审批和聊天授权的旧说明仅适用于 `approval` 兼容模式，不代表当前运行方式。

云端持久行为规则由本目录的 `cloud-hanako-AGENTS.md` 部署到：

```text
/root/Desktop/OH-WorkSpace/AGENTS.md
```

当前版本：

```text
1.4.1
```

`v0.7.1` 增加可选离线队列。工具调用传入 `queueIfOffline: true` 后，目标电脑离线时请求会保存在 VPS；设备重新上线后自动执行。使用 `local_device.queue` 查看结果，使用 `local_device.cancel_queued` 取消尚未执行的请求。

`v0.7.0` 为每台 Windows 保存稳定 `deviceId`，支持 `device://<deviceId>/C:/...` 文件与脚本路径，并提供 VPS 设备路由器和 `local_device.devices` 在线状态工具。当前默认设备为 `laptop-hl78935t`。

`v0.6.1` 为目录列表增加游标分页，为搜索增加 glob、排除规则、超时和节点预算，并新增 `watch / watch_events / unwatch`。Agent 可以等待 Windows 文件变化，不需要持续高频轮询整个目录。

`v0.6.0` 新增 `read_lines`、`append_text` 和 `apply_patch`。文本工具可以识别并保留 UTF-8 BOM、UTF-16LE BOM 和 UTF-16BE BOM；并发追加不会丢内容，精确补丁会同时校验 SHA256、旧文本和出现次数。

`v0.5.3` 把 PowerShell/Python 执行迁移到独立隐藏 Runner。MCP 服务重启不会中断正在运行的任务，新进程会从磁盘恢复 Runner PID、状态、退出码和输出。状态 JSON 自动保留 `.bak`，损坏时会保存损坏副本并回退到最近备份；审计、后台服务和隧道日志会自动轮转。

`v0.5.2` 为所有文件变更操作增加路径级并发锁。覆盖已有文件时会在提交前再次验证 SHA256，并使用“旧文件备份 -> 新文件替换 -> 失败回滚”的提交过程，避免并发请求导致 `ENOENT` 或原文件丢失。

## 当前架构

```text
Windows 本地桥
  -> WebSocket 主动连接云端 /local-bridge/connect
    -> Cloud Hana LocalBridgeGateway
      -> 设备路由器 127.0.0.1:18786
        -> local_fs / local_exec / local_device
```

本机授权页面：

```text
http://127.0.0.1:8788/
```

状态页面只监听 Windows 回环地址。网页只从该接口读取当前电脑的设备 ID 和一次性认领信息。

## 默认授权

```text
local://OH-WorkSpace
C:\Users\30456\Desktop\OH-WorkSpace
模式：读写

local://Hanako-Local-FS-MCP-Bridge
C:\Users\30456\Desktop\Hanako-Local-FS-MCP-Bridge
模式：只读
```

其他 Windows 本地文件夹需要：

1. 云端调用 `local_fs.request_access`。
2. 用户在本机打开 `http://127.0.0.1:8788/`。
3. 选择只读或读写并批准。
4. 云端通过新生成的 `local://<root>` 路径访问。

云端不能自行批准请求。

## 本地命令执行

普通脚本优先使用单步工具：

```text
local_exec.execute
```

聊天示例：

```text
我授权你执行 C:\Users\30456\Documents\Scripts\backup.ps1
我授权你执行 D:\Tools\report.py，参数是 --month 2026-07
```

一次执行会锁定：

```text
运行时、脚本绝对路径、脚本 SHA256、参数、工作目录、超时时间
```

长任务可以使用异步工具：

```text
local_exec.request_run
local_exec.run
local_exec.job_status
local_exec.job_output
local_exec.cancel_job
```

## 后台任务

安装并启动自动恢复任务。计划任务通过 `wscript.exe` 隐藏启动，不显示 PowerShell 窗口；MCP watchdog 和云端 WebSocket 会在后台自动恢复：

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\install-background-service.ps1
```

状态：

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\status.ps1
```

卸载：

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\uninstall-background-service.ps1
```

打开授权页面：

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\open-approval.ps1
```

## 文档

- [操作手册](./OPERATION_MANUAL.md)
- [开发维护手册](./DEVELOPMENT_MANUAL.md)
- [脱敏云端部署指南](./CLOUD_DEPLOYMENT_GUIDE.md)
- [云端 WebSocket 主动连接架构](./CLOUD_WEBSOCKET_ARCHITECTURE.md)
- [Windows 安装、迁移与更新手册](./WINDOWS_INSTALLER_UPDATE_MANUAL.md)

包含活动密码和设备令牌的本机完整维护手册不会提交到 Git，详见
[`SECURITY.md`](./SECURITY.md)。

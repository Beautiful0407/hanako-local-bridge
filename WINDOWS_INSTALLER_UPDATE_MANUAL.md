# Hanako Local Bridge Windows 安装、迁移与更新手册

稳定版本：`1.4.9`

Rust 预发布版本：`2.0.0-alpha.2`

日期：`2026-07-19`

## 0. 当前发布状态

```text
稳定渠道：1.4.9，继续用于现有 Windows 电脑
Alpha 渠道：2.0.0-alpha.2，包含纯 Rust Bridge、Manager、Updater 和 Installer
云端设备路由器：2.0.0-alpha.2 Rust 版已经部署
```

不要把 `target\release` 中的 EXE 手工覆盖到 `%LOCALAPPDATA%\HanakoLocalBridge`。Alpha 测试应使用独立安装目录，正式迁移必须使用内嵌安装器或签名更新清单。

Alpha 2 发布文件：

```text
build\rust-release-alpha2\HanakoLocalBridge-Setup-2.0.0-alpha.2.exe
build\rust-release-alpha2\HanakoLocalBridge-2.0.0-alpha.2-win-x64.zip
build\rust-release-alpha2\update-manifest.json
```

Alpha 2 已验证：

```text
首次安装
同目录覆盖安装
桌面和开始菜单快捷方式
计划任务启动与恢复
卸载注册和后台卸载
Alpha 1 负载升级到 Alpha 2
配置、data、logs 和未知用户文件保留
签名、SHA256、大小校验和失败回滚
```

## 1. 目标

Windows 本地桥负责：

```text
读取和写入本地文件
执行 PowerShell / Python
保持稳定设备 ID
主动通过 WebSocket 连接云端 Hana
网页登录后自动认领当前电脑
断线后自动恢复
后台运行且不弹出 PowerShell 窗口
提供图形化检测、修复、设备认领和日志查看
```

正式安装不依赖桌面源码目录，也不要求目标电脑预装 Node.js。

## 2. 发布文件

开发目录运行：

```powershell
npm.cmd run build:installer
```

生成：

```text
release\HanakoLocalBridge-Setup-1.4.9.exe
release\HanakoLocalBridge-1.4.9-win-x64.zip
release\update-manifest.json
```

用途：

```text
Setup EXE：普通用户双击安装
ZIP：更新器下载或离线更新
Manifest：声明版本、ZIP 地址、大小、SHA256 和 RSA-SHA256 签名
```

## 3. 默认安装布局

```text
%LOCALAPPDATA%\HanakoLocalBridge\
  config.json
  data\
  logs\
  manager\
    HanakoBridgeManager.exe
    App.xbf
    MainWindow.xbf
    HanakoBridgeManager.pri
  runtime\node.exe
  lib\
  scripts\
  manager-command.ps1
  manager-core.ps1
  manager-ui.ps1
  server.cjs
  run-local-fs-hidden.vbs
  run-local-fs-service.ps1
  run-reverse-tunnel-hidden.vbs
  run-reverse-tunnel-service.ps1
  status.ps1
  repair.ps1
  update.ps1
  uninstall-background-service.ps1
```

升级时必须保留：

```text
config.json
data\
logs\
```

其他文件可以由新版本覆盖。

`CLOUD_HANA_AGENT_MAINTENANCE_MANUAL.md` 是仅供本机保存的私有手册，不进入 ZIP、Setup EXE 或正式安装目录。`v1.3.1` 覆盖升级会删除旧版本误装到安装目录中的副本。

## 4. 新电脑安装

1. 把 `HanakoLocalBridge-Setup-1.4.9.exe` 放到目标电脑。
2. 双击安装器。
3. 在图形窗口确认设备名、设备 ID、本地文件根目录和云端 WebSocket URL。
4. 点击 `Install / Repair`，等待成功提示。
5. 从开始菜单打开 `Hanako Local Bridge Manager`。
6. 在 `云端设备` 页面输入 Hana 网页访问密钥并认领本机。

旧版本可以直接覆盖安装。安装器会保留：

```text
config.json
data\
logs\
```

并更新 WinUI 3 管理器、WinForms 回退管理器、后台服务脚本和自带 Node 运行时。目标电脑不需要另装 .NET 或 Windows App Runtime。
7. 运行 `status.ps1`，确认 `cloud.status` 为 `active`。

默认值：

```text
MCP：127.0.0.1:8787
状态页：127.0.0.1:8788
云端 WebSocket：wss://154-201-69-202.sslip.io/local-bridge/connect
信任模式：full
```

多电脑部署时，每台电脑必须使用不同的：

```text
device.id
```

设备会在认领成功后自动注册到 VPS 设备路由器，不再手动编辑 `devices.json`。

## 5. 从桌面源码版迁移

使用默认任务名安装时，安装器会检查：

```text
Hanako Local FS MCP
Hanako Local FS Tunnel
```

它从任务动作中识别旧安装目录，停止旧 watchdog，并迁移：

```text
config.json
data\
logs\
```

旧桌面目录不会被删除，可作为人工回滚副本。迁移后任务动作应指向：

```text
%LOCALAPPDATA%\HanakoLocalBridge\run-local-fs-hidden.vbs
%LOCALAPPDATA%\HanakoLocalBridge\run-reverse-tunnel-hidden.vbs
```

## 6. config.json

主要字段：

```json
{
  "device": {
    "id": "laptop-hl78935t",
    "name": "LAPTOP-HL78935T"
  },
  "filesystem": {
    "host": "127.0.0.1",
    "port": 8787,
    "approvalPort": 8788,
    "trustMode": "full",
    "roots": []
  },
  "storage": {
    "dataDir": "data",
    "logDir": "logs"
  },
  "cloud": {
    "enabled": true,
    "url": "wss://154-201-69-202.sslip.io/local-bridge/connect",
    "reconnectMinSeconds": 3,
    "reconnectMaxSeconds": 60,
    "heartbeatSeconds": 25
  },
  "tunnel": {
    "enabled": false,
    "server": "154.201.69.202",
    "user": "root",
    "localPort": 8787,
    "remoteHost": "127.0.0.1",
    "remotePort": 18787,
    "identityFile": ""
  },
  "service": {
    "taskPrefix": "Hanako Local FS",
    "restartDelaySeconds": 3,
    "tunnelRetryMinSeconds": 5,
    "tunnelRetryMaxSeconds": 60,
    "tunnelHealthSeconds": 30
  },
  "update": {
    "manifest": "",
    "channel": "stable"
  }
}
```

修改配置：

```powershell
cd $env:LOCALAPPDATA\HanakoLocalBridge
powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File .\configure.ps1
powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File .\repair.ps1 -NonInteractive
```

## 7. 无感后台运行

计划任务动作只启动：

```text
wscript.exe //B //NoLogo <hidden-launcher.vbs>
```

VBS 再使用：

```text
-NoLogo
-NoProfile
-NonInteractive
-ExecutionPolicy Bypass
-WindowStyle Hidden
```

启动 PowerShell watchdog。

每个安装目录的 MCP 和 Tunnel 分别持有命名 Mutex，因此重复登录、重复点击启动或任务重复触发不会产生多套服务。

默认 WebSocket 模式只运行 MCP watchdog。Tunnel 任务为升级、卸载和旧配置兼容而保留，`cloud.enabled=true` 时不会建立 SSH 连接。

恢复策略：

```text
Node 异常退出：默认 3 秒后重启
本地 MCP 未恢复：隧道暂不连接
WebSocket 断开：3 至 60 秒指数退避
WebSocket 恢复：使用持久设备凭证自动认证
云端或网络重启：不要求重新输入网页登录密钥
```

## 8. 状态与修复

检查：

```powershell
cd $env:LOCALAPPDATA\HanakoLocalBridge
powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File .\status.ps1
```

应看到：

```text
Local MCP health: 200
Local status UI: 200
Hanako Local FS MCP: Running
Hanako Local FS Tunnel: Ready
Cloud WebSocket status: active
```

修复任务和启动项：

```powershell
powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File .\repair.ps1 -NonInteractive
```

仅停止：

```powershell
powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File .\stop.ps1
```

## 9. 更新

本地清单：

```powershell
powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File .\update.ps1 `
  -Manifest "D:\Releases\update-manifest.json"
```

URL 清单：

```powershell
powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File .\update.ps1 `
  -Manifest "https://example.com/hanako/update-manifest.json" `
  -RememberManifest
```

更新器会：

```text
读取 manifest
比较版本
下载或复制 ZIP
校验 SHA256
校验 ZIP 内 package.json 版本
停止当前任务和进程
停止安装目录中正在运行的 WinUI 管理器
替换程序与自带 Node
保留 config/data/logs
重新注册并启动隐藏任务
```

同版本测试或修复可增加：

```powershell
-Force
```

## 10. 备份与迁移到另一台电脑

至少备份：

```text
config.json
data\
```

希望保留排错和历史执行信息时再备份：

```text
logs\
```

恢复顺序：

1. 在新电脑安装相同或更高版本。
2. 停止本地桥。
3. 覆盖 `config.json` 和 `data\`。
4. 检查本地路径、设备 ID 和 `cloud.url` 是否适用于新电脑。
5. 运行 `repair.ps1 -NonInteractive`。
6. 在新电脑登录 Hana 网页完成认领。
7. 在 VPS 设备路由器确认设备 ID 为在线。

同一设备迁移时保留 `data\device.json` 和 `data\cloud-identity.json`，云端会继续识别为原设备且不需要重新认领。把同一份 `cloud-identity.json` 同时复制到两台正在运行的电脑会造成连接互相替换，不能这样使用。

## 11. 卸载

Windows“已安装的应用”中运行卸载，或执行：

```powershell
powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass `
  -File .\uninstall-background-service.ps1 `
  -RemoveInstall `
  -KeepData
```

`-KeepData` 会先把配置、数据和日志备份到：

```text
%USERPROFILE%\Documents\HanakoLocalBridgeBackup
```

## 12. 开发与发布验证

构建机要求：

```text
Node.js 22+
.NET SDK 10
Windows 10 2004 或更高版本
```

目标电脑不需要预装 Node.js、.NET 或 Windows App Runtime；正式包同时携带 Node 和 WinUI/.NET 自包含运行时。

完整功能测试：

```powershell
npm.cmd test
```

管理命令 JSON 边界测试：

```powershell
npm.cmd run test:manager-command
```

构建：

```powershell
npm.cmd run build:manager
npm.cmd run build:installer
```

真实 EXE 烟雾测试：

```powershell
npm.cmd run test:installer
```

烟雾测试必须验证：

```text
生产任务状态不变化
EXE 能安装到临时目录
自带 Node 可运行
WinUI 管理器包含 EXE、XBF 和 PRI
WinUI 管理器 --smoke-test 返回 0
run-manager.vbs 能启动原生管理器
运行中的原生管理器不会阻塞覆盖更新
任务动作使用 wscript.exe
PowerShell watchdog 带 WindowStyle Hidden
强杀 Node 后 PID 自动更换且 health 恢复
更新后 data 标记文件仍存在
测试任务和临时目录被清理
```

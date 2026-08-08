# Hanako Local Bridge Rust 迁移说明

## 状态

Rust 实现当前版本为 `2.0.3`。旧版 Node.js / PowerShell / VBS / WinUI 实现已在 alpha.22 移除，仓库现在只包含 Rust 工作区。如需回滚旧实现，可从 Git 历史恢复。

## 为什么用 Rust

旧版安装包捆绑了 Node.js、PowerShell/VBS watchdog 逻辑和自包含的 .NET/Windows App SDK 管理器，这些实现在 `2.0.0-alpha.22` 中已移除。Rust 设计现在产出多个 Windows 运行时可执行文件加一个引导安装器，并复用受支持 Windows 系统上已有的 WebView2 运行时。这些是同一个 Hanako Local Bridge 产品内部的角色，不是独立产品：用户拿到的是同一个安装器、同一个管理器入口、同一个配置模型、同一个版本、同一条更新/修复链路。

从重建的 Windows x64 Alpha 12 发布版实测：

```text
hanako-bridge.exe       6,283,776 bytes
hanako-manager.exe      2,299,904 bytes
hanako-maintenance.exe  5,750,784 bytes
runtime ZIP             6,705,761 bytes
embedded installer      8,930,304 bytes
```

对比：稳定版 `1.4.9` 安装器约 `95.91 MiB`。云端主机上，Rust 设备路由器观测期后占用约 `5.1 MiB`，被替换的 Node 路由器约 `48.8 MiB`。

## 工作区结构

```text
Cargo.toml
Cargo.lock
crates/
  hanako-bridge-core/
apps/
  hanako-bridge/
  hanako-device-router/
  hanako-installer/
  hanako-manager/
  hanako-updater/          # 生成 hanako-maintenance.exe
tests/
  rust-integration.test.cjs
  rust-device-router.test.cjs
  rust-audit.test.cjs
  rust-recovery.test.cjs
  rust-installer-smoke.ps1
  rust-update-smoke.ps1
```

### `hanako-bridge-core`

- 将既有 JSON 配置与默认值做深合并
- 展开 `%INSTALLDIR%` 及其他环境变量
- 迁移旧版云与更新地址
- 持久化稳定的设备身份
- 原子写入 JSON，带 `.bak` 恢复与损坏文件保留
- 解析 `local://`、`device://`、别名与已授权的绝对路径

### `hanako-bridge`

- 使用 Axum 提供带 Token 保护的 MCP 端点与本地管理器 API
- 注册全部 70+ 个本地文件、执行与自动化工具（local_fs/local_exec/nuphus）
- 支持 full-trust 与 approval 两种模式
- 读写 UTF-8、UTF-16LE、UTF-16BE，并保留 BOM 状态
- 并发写入使用原子替换与 SHA256 前置校验
- 支持全文内容搜索（关键词/正则）、事务性批量操作（batch）、操作历史查询、可恢复回收站（list/restore/clear）、授权根动态管理（roots_add/remove）、分块二进制追加（append_base64）、图片分块、轮询 watch、复制、移动与精确补丁
- 通过隔离的 job runner 执行已授权的 PowerShell 与 Python 脚本
- 持久化任务，支持超时与取消，服务重启后恢复 runner 结果
- 复用 Ed25519 云端身份，带心跳与退避重连云端 WebSocket
- 安装或修复直接启动 Rust 服务的计划任务

### `hanako-manager`

- 使用 Winit、Wry、WebView2 与 `tray-icon`
- 展示概览、诊断、已配置根目录、日志与设置
- 最小化或关闭时隐藏到系统托盘
- 托盘双击恢复窗口，提供打开与退出菜单命令
- 本地管理器端点不可用时，启动或修复 Rust 服务
- 发布构建使用 Windows GUI 子系统，不显示控制台窗口

### `hanako-maintenance.exe`

- 检查本地或 HTTPS 清单，验证旧版 RSA XML 公钥格式
- 使用 HTTPS 下载安装包，校验 SHA256 与大小，拒绝 HTTP
- 解压 ZIP 带目录穿越防护
- 事务化应用安装包，保留 `config.json`、`data`、`logs` 与未知用户文件
- 只按上一份受管清单删除过期文件
- 以独立 worker 运行，记录 `data/update-state.json`，替换失败自动回滚
- 提供 Rust `pack` 命令，用于生成仅含运行时的 ZIP 包与更新清单

### `HanakoLocalBridge-Setup.exe`

- 构建时内嵌 Rust 运行时 ZIP
- 按用户安装到 `%LOCALAPPDATA%\HanakoLocalBridge`，无需管理员提权
- 使用 Rust 创建桌面与开始菜单快捷方式
- 注册按用户的卸载条目，使用独立的 Rust 卸载 worker
- 覆盖安装与在线更新使用同一套负载事务

### `hanako-device-router`

- 在 Linux 上替代 `cloud/device-router.cjs`
- 保留 `/health`、`/mcp` 与 `/devices/register`
- 保持 70+ 个工具面、`device://<deviceId>/...` 选择、Token 转发与离线队列文件
- 与 Node 路由器使用相同的 JSON 配置、缓存与队列路径

## 工具链

已验证工具链：

```text
Rust/Cargo: 1.97.1
Visual Studio Build Tools: C:\BuildTools2026
Windows SDK: 10.0.26100.0
```

Cargo 构建前需在 PowerShell 中加载 MSVC 环境：

```powershell
$vcvars = 'C:\BuildTools2026\VC\Auxiliary\Build\vcvars64.bat'
$lines = cmd.exe /d /s /c "`"$vcvars`" >nul && set"
foreach ($line in $lines) {
  if ($line -match '^([^=]+)=(.*)$') {
    [Environment]::SetEnvironmentVariable($Matches[1], $Matches[2], 'Process')
  }
}
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"
```

## 构建与测试

运行严格验证链：

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --workspace --release
cargo build --workspace
$env:HANAKO_RUST_BRIDGE_EXE = (Resolve-Path 'target\release\hanako-bridge.exe').Path
node tests\rust-integration.test.cjs
node tests\rust-audit.test.cjs
node tests\rust-recovery.test.cjs
node tests\rust-device-router.test.cjs
powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File tests\rust-update-smoke.ps1
powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File tests\rust-installer-smoke.ps1
$entry = Start-Process target\release\hanako-bridge.exe -ArgumentList '--smoke-test' -Wait -PassThru
if ($entry.ExitCode -ne 0) { throw "Unified product entry smoke test failed" }
```

集成测试使用随机回环端口与临时根目录。安装器冒烟测试会通过 VBS 启动一个分离的旧版 Node 进程，验证 Rust 版接管，检查快捷方式指向 `hanako-bridge.exe`，断言安装后的 Bridge PE 子系统为 Windows GUI，检查一分钟计划任务触发器，调用已安装管理器的修复 API，验证覆盖安装与重复产品入口的单实例行为，关闭管理器后确认 Bridge 保持健康，强制终止 Bridge 后等待不同进程 ID 无窗口恢复，最后卸载。更新冒烟测试安装一份 Alpha 11 负载，注入当前 Alpha 12 维护二进制，验证签名更新交接、快捷方式/卸载迁移与回滚。维护单元测试刻意中断 HTTP 响应体并要求 Range 续传；显式探针会忽略并验证真实发布 URL、大小与 SHA256。

## 发布打包

生产私钥不在仓库内：

```text
%USERPROFILE%\.hanako-update-signing\private-key.xml
```

构建签名预发布版：

```powershell
cargo build --workspace --release

target\release\hanako-maintenance.exe pack `
  --binaries target\release `
  --output build\rust-release-alpha12 `
  --public-key update-public-key.xml `
  --version 2.0.0-alpha.12 `
  --channel alpha `
  --package-url HanakoLocalBridge-2.0.0-alpha.12-win-x64.zip `
  --signing-key "$env:USERPROFILE\.hanako-update-signing\private-key.xml" `
  --notes "Hanako Local Bridge Rust 2.0.0-alpha.12: resumable remote update downloads"

$env:HANA_INSTALLER_PAYLOAD = (
  Resolve-Path 'build\rust-release-alpha12\HanakoLocalBridge-2.0.0-alpha.12-win-x64.zip'
).Path
cargo build -p hanako-bootstrap --release
```

## 预览

启动预览前，将 `HANA_LOCAL_BRIDGE_CONFIG` 指向隔离的配置：

```powershell
$env:HANA_LOCAL_BRIDGE_CONFIG = 'C:\path\to\preview\config.json'
target\debug\hanako-bridge.exe --service
```

打开：

```text
http://127.0.0.1:<approvalPort>/manager/
```

不要将开发预览指向生产数据目录。

## 兼容性

Rust 服务读取既有 camelCase 的 `config.json` 结构，深合并时保留未知配置字段。它沿用既有的存储文件名与云端身份格式，因此最终安装器可以在不强制每台设备重新认领的情况下完成迁移。

受支持的兼容路径：

```text
local://<alias>/...
device://<deviceId>/C:/...
C:\absolute\path
```

最终迁移必须验证这些文件在覆盖安装与在线更新后的状态：

```text
config.json
data/device.json
data/cloud-identity.json
data/access-control.json
data/pending-requests.json
data/approval-token.txt
data/execution-authorizations.json
data/execution-requests.json
data/jobs/
logs/
```

## 已验证部署

Linux 路由器在 Ubuntu 22.04 上使用 Rust `1.97.1` 构建，部署为：

```text
/opt/hanako-local-device-router/hanako-device-router
/etc/systemd/system/hanako-local-device-router.service
```

2026 年 7 月 19 日实测确认：

```text
router version: 2.0.0-alpha.2
tools: 34
online devices: your-laptop-id, 5cd5469l5j
local_device.devices: success
local_fs.roots routed to your-laptop-id: success
Hana web entry: HTTP 200 and connected UI
```

旧版 Node 脚本与仅 root 可读的时间戳备份仍保留在服务器上用于回滚。

## 剩余生产工作

1. 将签名后的 Alpha 安装器与清单发布到独立预发布频道
2. 在干净的 Windows 10 与 Windows 11 虚拟机上验证安装、托盘行为、更新、卸载与重启恢复
3. 在非主力设备上先做分阶段迁移，再向稳定设备群提供 Rust 安装器
4. 旧版 Node/PowerShell/VBS/WinUI 实现已在 alpha.22 移除；如将来需要回滚到旧技术栈，从 Git 历史恢复

## 发布规则

每个已提交的开发阶段都必须 bump 产品版本。不要从未提交的工作区发布安装器或更新清单，不要用 Alpha 构建覆盖稳定频道。

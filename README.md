# Hanako Local Bridge

> Windows 本地 MCP 桥：文件读写、PowerShell/Python 执行、多设备路由与签名自动更新。
> Windows-local MCP bridge for Hana Agent: filesystem access, script execution, device routing, and signed auto-updates.

Hanako Local Bridge 是云端 Hana Agent 与 Windows 电脑之间的本地桥。它把 Hana 的能力延伸到 Windows 本地文件系统与 PowerShell/Python 运行时，同时保持连接方向由本机主动发起、所有远程操作可审计、更新必须验签。

## 特性

**本地 MCP 服务（31 个工具）**

- 文件：读、写、追加、精确补丁（SHA256 并发校验）、搜索（glob/排除/超时）、目录游标分页、watch 文件变化、图片读取
- 执行：PowerShell / Python 单步执行与异步任务（运行时、脚本 SHA256、参数、工作目录、超时全锁定），任务在独立隐藏 Runner 中运行，MCP 服务重启不中断
- 设备：稳定 deviceId、`device://<deviceId>/C:/...` 路径、离线队列（`queueIfOffline`）、在线状态
- 写安全：路径级并发锁、覆盖前 SHA256 复核、旧文件备份 + 失败回滚

**云端接入**

- 本地桥主动 WebSocket/WSS 连接云端，无需 SSH 反向隧道
- Ed25519 设备身份 + 一次性 `claimToken` 认领，云端签发设备凭证
- Linux Device Router 支持多设备路由与在线/离线状态

**安全与更新**

- 本地 MCP 接口 Bearer Token、Origin/Host 防护、请求体上限
- 远程更新经 RSA 签名 + SHA256 + 大小三重校验，HTTP Range 断点续传
- 本地授权页只监听回环地址，访问密钥不落盘

**多语言实现**

- 稳定版：Node.js + PowerShell watchdog + WinUI 3 管理器（v1.4.9）
- Rust 2.0 预览：Bridge / Device Router / Manager / Maintenance / Installer 统一工作区，正在替换 Node.js 实现

## 架构

```text
Windows 本地桥
  -> WebSocket/WSS 主动连接云端 /local-bridge/connect
    -> Cloud Hana LocalBridgeGateway
      -> 设备路由器 (Linux, Rust)
        -> local_fs / local_exec / local_device
```

```text
Windows 侧组件：

  hanako-bridge      本地 MCP 服务（HTTP/WS，默认 8787）
  hanako-manager     WinUI 3 托盘管理器（自检、诊断、设备认领）
  hanako-maintenance 签名更新器（RSA 验签 + SHA256 + 断点续传）
  hanako-bootstrap   内嵌安装器（NSIS 向导 / 静默安装）
  watchdog           隐藏计划任务，异常退出自动重启
```

## 快速开始

### 本地构建

```bash
# Rust 工作区（Windows）
cargo build --workspace --release
cargo test --workspace

# 集成测试（需要本机桥在运行）
$env:HANAKO_RUST_BRIDGE_EXE = (Resolve-Path 'target\release\hanako-bridge.exe').Path
node tests\rust-integration.test.cjs
node tests\rust-audit.test.cjs
node tests\rust-recovery.test.cjs
node tests\rust-device-router.test.cjs

# Node.js 实现
npm install
npm run check
```

### 配置

以 [`config.example.json`](./config.example.json) 为模板创建 `config.json`，填入你自己的云端地址与端口：

```json
{
  "cloud": {
    "enabled": true,
    "url": "wss://your-server.example.com/local-bridge/connect"
  },
  "tunnel": {
    "server": "YOUR_SERVER_IP",
    "user": "root"
  },
  "update": {
    "manifest": "https://your-server.example.com/local-bridge/releases/update-manifest.json"
  }
}
```

### 安装为 Windows 后台服务

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\install-background-service.ps1
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\status.ps1
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\uninstall-background-service.ps1
```

### 构建安装器

```powershell
# 原生安装器（NSIS 向导 + 静默安装）
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\build-installer.ps1
```

### 发布更新包

```powershell
cargo build --workspace --release
target\release\hanako-maintenance.exe pack `
  --binaries target\release `
  --output build\release `
  --public-key update-public-key.xml
```

发布清单模板见 [`update-manifest.example.json`](./update-manifest.example.json)，签名公钥见 [`update-public-key.xml`](./update-public-key.xml)。

## 文档

| 文档 | 内容 |
|---|---|
| [OPERATION_MANUAL.md](./OPERATION_MANUAL.md) | 安装、配置与日常运维 |
| [DEVELOPMENT_MANUAL.md](./DEVELOPMENT_MANUAL.md) | 开发、测试与发布流程 |
| [CLOUD_WEBSOCKET_ARCHITECTURE.md](./CLOUD_WEBSOCKET_ARCHITECTURE.md) | 云端主动连接与认领架构 |
| [CLOUD_DEPLOYMENT_GUIDE.md](./CLOUD_DEPLOYMENT_GUIDE.md) | 云端部署（脱敏） |
| [WINDOWS_INSTALLER_UPDATE_MANUAL.md](./WINDOWS_INSTALLER_UPDATE_MANUAL.md) | 安装器、迁移与更新 |
| [RUST_MIGRATION.md](./RUST_MIGRATION.md) | Rust 2.0 迁移状态 |
| [SECURITY.md](./SECURITY.md) | 安全边界与发布前检查 |

## 安全

- 本仓库只包含源码与脱敏文档，严禁提交运行时状态与凭据，详见 [`SECURITY.md`](./SECURITY.md)
- 默认部署中，本地桥以 `full` 信任模式工作，权限边界由部署方通过 `config.json` 的 roots 与 trustMode 控制
- 发现安全问题请直接开 Issue，不要公开张贴凭据或私钥

## License

[Apache-2.0](./LICENSE)

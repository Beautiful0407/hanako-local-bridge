# Hanako Local Bridge

> Windows 本地 MCP 桥：文件读写、PowerShell/Python 执行、多设备路由与签名自动更新。

Hanako Local Bridge 是云端 Hana Agent 与 Windows 电脑之间的本地桥。它把 Hana 的能力延伸到 Windows 本地文件系统与 PowerShell/Python 运行时，同时保持连接方向由本机主动发起、所有远程操作可审计、更新必须验签。

本项目使用 Rust 实现（2.0.x），单安装器、单管理器、单配置模型、单版本、单更新链路。

## 特性

**本地 MCP 服务（70+ 工具）**

- 文件：读、写、追加、精确补丁（SHA256 并发校验）、目录游标分页、watch 文件变化、图片读取
- 搜索：按文件名/glob 搜索 + **全文内容搜索**（关键词/正则，UTF-8/UTF-16 自适应，二进制自动跳过）
- 批量：`local_fs.batch` **事务性批量操作**（copy/move/delete 要么全成功要么全回滚）
- 历史：`local_fs.history` 查询操作审计（按工具/成败/时间过滤，含触碰路径）
- 回收站：`local_fs.trash_list` / `trash_restore` / `trash_clear` 可恢复删除（记录原路径）
- 权限：`local_fs.roots_add` / `roots_remove` 运行时动态管理授权目录（持久化、立即生效）
- 分块传输：`local_fs.append_base64` 大文件分块追加 + SHA256 整体校验
- 执行：PowerShell / Python 单步执行与异步任务（运行时、脚本 SHA256、参数、工作目录、超时全锁定），任务在独立隐藏 Runner 中运行，MCP 服务重启不中断
- 桌面/浏览器自动化：截图、视觉理解、鼠标键盘、窗口控制、浏览器导航/点击/填表/取数（nuphus 系列）
- 设备：稳定 deviceId、`device://<deviceId>/C:/...` 路径、离线队列（`queueIfOffline`）、在线状态
- 写安全：路径级并发锁（固定顺序防死锁）、覆盖前 SHA256 复核、旧文件备份 + 失败回滚

**云端接入**

- 本地桥主动 WebSocket/WSS 连接云端，无需 SSH 反向隧道
- Ed25519 设备身份 + 一次性 `claimToken` 认领，云端签发设备凭证
- Linux Device Router 支持多设备路由与在线/离线状态
- 管理器一键认领引导（打开认领页 → 登录 Hana → 自动绑定）

**安全与更新**

- 本地 MCP 接口 Bearer Token、Origin/Host 防护、请求体上限
- 远程更新经 RSA 签名 + SHA256 + 大小三重校验，HTTP Range 断点续传
- 本地授权页只监听回环地址，访问密钥不落盘
- 全部工具调用写入审计日志（`logs/mcp-audit.jsonl`，10MB 轮转）

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
  hanako-manager     WinUI WebView2 托盘管理器（自检、诊断、设备认领）
  hanako-maintenance 签名更新器（RSA 验签 + SHA256 + 断点续传）
  hanako-bootstrap   内嵌安装器（NSIS 向导 / 静默安装）
  计划任务           每分钟触发，异常退出自动重启
```

## 快速开始

### 构建与测试（Windows，需要 MSVC 工具链）

```powershell
# 严格验证链
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --workspace --release

# 集成测试（需要本机桥在运行）
$env:HANAKO_RUST_BRIDGE_EXE = (Resolve-Path 'target\release\hanako-bridge.exe').Path
node tests\rust-integration.test.cjs
node tests\rust-audit.test.cjs
node tests\rust-recovery.test.cjs
node tests\rust-device-router.test.cjs
powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File tests\rust-update-smoke.ps1
powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File tests\rust-installer-smoke.ps1
```

### 配置

以 [`config.example.json`](./config.example.json) 为模板创建 `config.json`，填入你自己的云端地址与端口：

```json
{
  "cloud": {
    "enabled": true,
    "url": "wss://your-server.example.com/local-bridge/connect"
  },
  "update": {
    "manifest": "https://your-server.example.com/local-bridge/releases/update-manifest.json",
    "channel": "stable"
  }
}
```

未填写的字段（`tunnel`、`service`、`storage`、重连/心跳参数等）会自动使用内置默认值；`tunnel` 仅为旧版 SSH 反向隧道兼容保留，新部署无需配置。

> `your-server.example.com` 只是安装示例，并不是可连接的云端。请在管理器的“连接到云”引导中填写真实 `wss://<你的域名>/local-bridge/connect` 地址；2.0.8 会将该示例地址明确显示为“未配置”，不会再把它报告为 DNS 连接异常。

### 接入官方 OpenHanako 云端（远程 MCP）

桥自带标准 MCP HTTP 端点（`POST /mcp`，Bearer token 鉴权），官方 OpenHanako（`liliMozi/openhanako`，0.446.6+）支持把桥注册为**远程 MCP connector**（streamable-http / 2026-07-28 stateless 协议），让官方云端的 AI 直接调用桥的全部工具（local_fs.* / local_exec.* / nuphus.*）。

**桥侧**（本地电脑）：

1. 确认桥在运行，MCP 端点监听在 `http://127.0.0.1:8787/mcp`（默认端口可用 `LOCAL_FS_MCP_HOST/PORT` 覆盖）。
2. 取 MCP token：`<安装目录>\data\approval-token.txt`（首次启动自动生成）。
3. 把桥的 MCP 端点暴露到公网（任选一种）：
   - 反向代理（nginx / Caddy）：`location /mcp { proxy_pass http://127.0.0.1:8787; proxy_http_version 1.1; }`，配 TLS；
   - frp / cloudflared / ngrok 等隧道，把本地 8787 映射为公网 HTTPS 地址；
   - 公网地址形如 `https://你的域名/mcp`。

**官方云端侧**（服务器）：

1. 登录官方 OpenHanako 管理界面，进入 **MCP 设置**；
2. 添加 connector：`transport = streamable-http`，`url = https://你的域名/mcp`，`authorizationToken = <approval-token.txt 内容>`；
3. 保存并启动 connector，工具列表自动刷新（80+ 工具）。
4. 在会话中调用 `local_fs.*` / `local_exec.*` 即可操作桥所在电脑的文件。

**安全提示**：仅暴露 `/mcp` 路径；token 按需轮换（删除 `approval-token.txt` 重启桥即可重新生成）；桥的 roots/approval/审计权限控制对远程调用同样生效。

> 注意：此模式面向**官方 OpenHanako 云端**（远端 MCP）。**私人云端线**（`openhanako-cloud`）使用 `wss://…/local-bridge/connect` 主动连接 + 认领协议，两者独立、可共存。

### 安装为 Windows 后台服务

Rust 版安装器会创建计划任务并直接启动 `hanako-bridge.exe`，无需额外服务脚本。安装包由 [`apps/hanako-installer`](./apps/hanako-installer) 生成。

### 构建安装器（NSIS 向导外壳）

```powershell
makensis /DVERSION=<ver> /DSETUP_EXE=<abs path to HanakoLocalBridge-Setup.exe> `
         /DOUT_FILE=<abs output path> installer\wizard.nsi
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
| [RUST_MIGRATION.md](./RUST_MIGRATION.md) | 迁移状态、组件说明与发布流程 |
| [CLOUD_WEBSOCKET_ARCHITECTURE.md](./CLOUD_WEBSOCKET_ARCHITECTURE.md) | 云端主动连接与认领架构 |
| [CLOUD_DEPLOYMENT_GUIDE.md](./CLOUD_DEPLOYMENT_GUIDE.md) | 云端部署 |
| [SECURITY.md](./SECURITY.md) | 安全边界与发布前检查 |
| [CHANGELOG.md](./CHANGELOG.md) | 版本变更记录 |

## 安全

- 本仓库只包含源码与脱敏文档，严禁提交运行时状态与凭据，详见 [`SECURITY.md`](./SECURITY.md)
- 默认部署中，本地桥以 `full` 信任模式工作，权限边界由部署方通过 `config.json` 的 roots 与 trustMode 控制
- 发现安全问题请直接开 Issue，不要公开张贴凭据或私钥

## License

[Apache-2.0](./LICENSE)

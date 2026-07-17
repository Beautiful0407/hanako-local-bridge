# Hanako Local Bridge 主动连接云端方案

版本：`1.2.0`

日期：`2026-07-17`

## 1. 目标

本方案解决旧版 SSH 反向隧道的三个主要问题：

```text
新电脑必须生成 SSH 密钥并手动写入 VPS
每台电脑必须分配独立的反向隧道端口
浏览器不能可靠判断当前访问者正在使用哪台电脑
```

新架构改为：

```text
Windows 本地桥主动连接云端 Hana
网页登录后自动发现并认领当前电脑
云端按稳定 deviceId 把文件和脚本调用路由回正确电脑
```

用户仍然通过 Hana 网页的访问密钥登录。访问密钥只交给 Hana 登录接口，不发送给本地桥，也不写入本地桥配置。

## 2. 总体架构

```text
Windows Hanako Local Bridge 1.2.0
  |
  | ws://154.201.69.202/local-bridge/connect
  | 生产环境应升级为 wss://
  v
Cloud Hana 0.401.11 LocalBridgeGateway
  |
  | 127.0.0.1 内部 HTTP
  v
Device Router 0.8.0
  |
  v
Hana local_fs / local_exec / local_device 工具
```

浏览器认领链路：

```text
浏览器访问 http://154.201.69.202/desktop/
  -> 输入 Hana 访问密钥
  -> Hana 创建已认证网页会话
  -> 网页访问 http://127.0.0.1:8788/api/client-identity
  -> 读取当前电脑 deviceId、claimToken 和公钥指纹
  -> POST /api/local-bridge/claim
  -> 云端签发独立设备凭证
  -> 本地桥保存凭证并进入 active
```

## 3. 首次安装和首次认领

1. 用户在 Windows 双击 `HanakoLocalBridge-Setup-1.2.0.exe`。
2. 安装器生成稳定 `deviceId`，默认来自 Windows 计算机名。
3. 本地桥首次启动时生成 Ed25519 密钥对和随机 `claimToken`。
4. 私钥、认领令牌和后续设备凭证保存在：

```text
%LOCALAPPDATA%\HanakoLocalBridge\data\cloud-identity.json
```

5. 本地桥主动连接云端 WebSocket，并用私钥签名随机 nonce，证明自己持有设备私钥。
6. 云端验证签名后把连接标记为 `pending_claim`。
7. 用户在同一台电脑的浏览器登录 Hana。
8. 网页从本机回环接口取得 `claimToken`，调用已认证的云端认领接口。
9. 云端校验登录权限、令牌和公钥指纹，签发只属于这台本地桥的设备凭证。
10. 本地桥保存设备凭证，以后重启后自动认证，不再要求用户重复认领。

## 4. 为什么网页登录密钥不能直接发给本地桥

网页登录密钥和设备凭证职责不同：

```text
网页登录密钥
  用于证明“这个网页用户可以进入 Hana”
  可能拥有聊天、设置或管理权限

本地桥设备凭证
  用于证明“这个 WebSocket 连接是已经认领的 Windows 设备”
  只授予本地文件桥所需的设备权限
```

将两者分离后，即使本地桥配置文件被读取，也不会泄露 Hana 的网页登录密钥。撤销某台设备时也不需要修改所有网页登录密码。

## 5. 自动重连

本地桥运行在原有隐藏 MCP watchdog 中：

```text
计划任务 -> wscript.exe -> 隐藏 PowerShell watchdog -> Node MCP
```

恢复行为：

```text
Node 异常退出：默认 3 秒后重启
WebSocket 断开：3 至 60 秒指数退避重连
网络恢复：自动重新认证
云端 Hana 重启：本地桥自动重连
Windows 重新登录：计划任务自动启动
```

旧的 Tunnel 计划任务为了覆盖升级和卸载兼容仍可存在，但在 `cloud.enabled=true`、`tunnel.enabled=false` 时不会建立 SSH 连接。

## 6. 多电脑路由

每台电脑拥有独立：

```text
deviceId
Ed25519 密钥对
云端设备凭证
WebSocket 连接
```

网页每次提交消息前检测当前浏览器所在电脑，并把设备信息加入本轮上下文。Agent 调用本地工具时必须传入对应 `deviceId`，也可以使用：

```text
device://<deviceId>/C:/Users/name/file.txt
```

当检测到多台设备时，云端不再无条件回退到第一台电脑。这样在第二台电脑打开同一个 Hana 网址时，文件请求会路由到第二台电脑。

## 7. 云端接口

公网 WebSocket：

```text
GET /local-bridge/connect
```

已登录网页接口：

```text
POST /api/local-bridge/claim
GET  /api/local-bridge/devices
```

仅 VPS 回环可访问：

```text
POST /internal/local-bridge/devices/:deviceId/mcp
GET  /internal/local-bridge/devices/:deviceId/health
```

认领接口要求 `bridge.manage` scope 或 Studio Owner 身份。当前 Hana 网页访问密钥包含该 scope 时可以直接完成认领。

## 8. 本地状态接口

```text
http://127.0.0.1:8787/health
http://127.0.0.1:8788/api/client-identity
```

正常状态示例：

```json
{
  "ok": true,
  "version": "1.2.0",
  "cloud": {
    "status": "active",
    "claimToken": null,
    "cloudUrl": "ws://154.201.69.202/local-bridge/connect"
  }
}
```

常见状态：

```text
connecting       正在连接云端
authenticating   已连接，正在发送设备证明
pending_claim    等待已登录网页认领
active           已认领并可接收云端 MCP 调用
offline          网络断开，等待自动重连
error            配置或 WebSocket 运行时错误
```

## 9. 配置

`config.json` 的当前关键字段：

```json
{
  "cloud": {
    "enabled": true,
    "url": "ws://154.201.69.202/local-bridge/connect",
    "reconnectMinSeconds": 3,
    "reconnectMaxSeconds": 60,
    "heartbeatSeconds": 25
  },
  "tunnel": {
    "enabled": false
  }
}
```

旧配置没有 `cloud` 节点时，`lib/runtime-config.cjs` 会自动迁移：

```text
根据 tunnel.server 生成 ws://<server>/local-bridge/connect
启用 cloud
停用 tunnel
保留 deviceId、授权数据、任务数据和日志
```

## 10. 安全边界

```text
本地 MCP 和状态接口只监听 127.0.0.1
云端内部 MCP 转发接口只允许 VPS 回环请求
设备必须证明持有 Ed25519 私钥
首次认领同时校验 claimToken 和公钥指纹
设备凭证与 Hana 网页访问密钥分离
私钥文件按当前用户权限保存
```

当前公网入口还是明文 HTTP/WS，适合现阶段受控测试。正式长期使用应配置域名、TLS 和 Nginx WebSocket 转发，把入口改为：

```text
https://hana.example.com/
wss://hana.example.com/local-bridge/connect
```

## 11. 版本和部署对应关系

```text
Windows Bridge: 1.2.0
Cloud Hana:     0.401.11
Device Router: 0.8.0
Protocol:       1
```

三部分应作为同一阶段发布。只升级 Windows 桥但未部署云端网关时，本地状态会保持 `offline` 或 `pending_claim`；旧 SSH 兼容模式仍可手动启用用于回滚。

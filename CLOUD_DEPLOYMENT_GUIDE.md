# Hanako Local Bridge 云端部署指南

本指南为仓库安全版云端部署指南。它有意省略线上密码、访问密钥、设备凭证与私有基础设施备份。

## 组件

```text
Windows 本地桥：    2.0.8（Rust）
Cloud Hana：        当前兼容部署
设备路由器：        2.0.0-alpha.2（Rust）
```

Windows 桥主动连接云端：

```text
Windows 本地桥
  -> wss://your-server.example.com/local-bridge/connect
  -> Cloud Hana LocalBridgeGateway
  -> 设备路由器 127.0.0.1:18786
  -> local_fs / local_exec / local_device 工具
```

生产端点使用带受信证书的 HTTPS/WSS，并由 Certbot 自动续期。

## Windows 配置

以 `config.example.json` 为起点：

```json
{
  "cloud": {
    "enabled": true,
    "url": "wss://hana.example.com/local-bridge/connect",
    "reconnectMinSeconds": 3,
    "reconnectMaxSeconds": 60,
    "heartbeatSeconds": 25
  },
  "tunnel": {
    "enabled": false
  }
}
```

运行安装器（`HanakoLocalBridge-Setup.exe`）完成安装。安装器会创建计划任务并直接启动 `hanako-bridge.exe`，无需额外服务脚本。

检查本地状态：打开管理器（托盘图标）查看概览，或访问：

```text
http://127.0.0.1:<approvalPort>/manager/
```

预期状态：

```text
Bridge 版本：2.0.8
云端状态：active
后台任务：Running
```

## 首次设备认领

1. 安装并启动 Windows 桥
2. 在另一台电脑上打开 Hana 网页客户端
3. 使用 Hana 网页访问密钥登录
4. 浏览器读取本机回环身份端点
5. 已认证的浏览器认领待处理的桥连接
6. 桥保存独立的云端设备凭证

Hana 网页访问密钥永远不会被桥保存。

## 云端服务

为 Linux x86-64 构建 `hanako-device-router`，部署到 root 所有的应用目录，并将 `cloud/hanako-local-device-router.service` 作为 systemd 模板。

路由器只允许监听：

```text
127.0.0.1:18786
```

生产布局：

```text
/opt/hanako-local-device-router/hanako-device-router
/opt/hanako-local-device-router/devices.json
/opt/hanako-local-device-router/tools-cache.json
/opt/hanako-local-device-router/offline-queue.json
/etc/systemd/system/hanako-local-device-router.service
```

替换 Node 路由器前，先对应用目录与 systemd 单元做仅 root 可读的时间戳备份。将 Rust 二进制暂存为 `hanako-device-router.new`，用 `file`、`ldd` 与 `sha256sum` 验证后，停止旧服务并原子重命名暂存二进制。

切换后：

```bash
systemctl daemon-reload
systemctl restart hanako-local-device-router
systemctl is-active hanako-local-device-router
curl -fsS http://127.0.0.1:18786/health
```

回滚时恢复保存的 Node 单元并重启同一服务名。在 Rust 部署完成观测期之前，保留 `device-router.cjs`、`devices.json`、`tools-cache.json` 与 `offline-queue.json`。

云端 Hana 进程暴露：

```text
GET  /local-bridge/connect
POST /api/local-bridge/claim
GET  /api/local-bridge/devices
```

内部转发路由必须仅限回环：

```text
POST /internal/local-bridge/devices/:deviceId/mcp
GET  /internal/local-bridge/devices/:deviceId/health
```

## 反向代理

反向代理直接提供签名更新产物，并为 Hana 应用保留 WebSocket 升级：

```nginx
location = /local-bridge/releases { return 308 /local-bridge/releases/; }

location ^~ /local-bridge/releases/ {
    alias /var/www/hanako-local-bridge-releases/;
    autoindex off;
    default_type application/octet-stream;
    add_header Cache-Control "no-cache" always;
    limit_except GET HEAD { deny all; }
}

location / {
    proxy_pass http://127.0.0.1:14500;
    proxy_http_version 1.1;
    proxy_set_header Host $host;
    proxy_set_header X-Real-IP $remote_addr;
    proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
    proxy_set_header X-Forwarded-Proto $scheme;
    proxy_set_header Upgrade $http_upgrade;
    proxy_set_header Connection "upgrade";
    proxy_read_timeout 3600s;
    proxy_send_timeout 3600s;
    proxy_buffering off;
}
```

## 验证

逐层验证：

```text
公开更新清单与 ZIP 无需 GitHub 认证即返回 HTTP 200
Windows /health 报告版本与 cloud.status=active
云端 /api/local-bridge/devices 报告设备在线
路由器 /health 报告版本、34 个工具与所有预期设备在线
local_device.devices 经路由器调用成功
local_fs.roots 带显式 deviceId 调用成功
local_fs.read_text 经路由器调用成功
停止 Windows 服务后路由器报告离线
启动计划任务后自动恢复 active/online
```

## 备份

本机备份：

```text
config.json
data/
logs/
```

服务器备份：

```text
Hana 主目录数据
local-bridge-devices.json
设备路由器配置与离线队列
Cloud Hana 源码补丁与运行产物
```

永远不要把同一个 `cloud-identity.json` 复制到两台会同时运行的电脑。它们会共享同一身份并互相顶掉对方的活跃连接。

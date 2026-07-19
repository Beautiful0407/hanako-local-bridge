# Hanako Local Bridge Cloud Deployment Guide

This is the repository-safe cloud deployment guide. It intentionally omits
live passwords, access keys, device credentials, and private infrastructure
backups.

## Components

```text
Windows stable bridge: 1.4.9
Windows Rust preview:  2.0.0-alpha.3
Cloud Hana:            current compatible deployment
Device Router:         2.0.0-alpha.2 (Rust)
```

The Windows bridge actively connects to the cloud:

```text
Windows Local Bridge
  -> wss://154-201-69-202.sslip.io/local-bridge/connect
  -> Cloud Hana LocalBridgeGateway
  -> Device Router on 127.0.0.1:18786
  -> local_fs / local_exec / local_device tools
```

The production endpoint uses HTTPS/WSS with a trusted certificate and automatic Certbot renewal.

## Windows Configuration

Start from `config.example.json`:

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

Install the background task:

```powershell
powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass `
  -File .\install-background-service.ps1
```

Check local status:

```powershell
powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass `
  -File .\status.ps1
```

Expected state:

```text
Bridge version: 1.4.1
Cloud status: active
MCP task: Running
Legacy Tunnel task: Ready
```

## First Device Claim

1. Install and start the Windows bridge.
2. Open the Hana web client on the same computer.
3. Sign in with a Hana web access key.
4. The browser reads the loopback client identity endpoint.
5. The authenticated browser claims the pending bridge connection.
6. The bridge stores a separate cloud device credential.

The Hana web access key is never stored by the bridge.

## Cloud Services

Build `hanako-device-router` for Linux x86-64, deploy it to a root-owned
application directory, and use `cloud/hanako-local-device-router.service` as
the systemd template.

The router must only listen on:

```text
127.0.0.1:18786
```

Production layout:

```text
/opt/hanako-local-device-router/hanako-device-router
/opt/hanako-local-device-router/devices.json
/opt/hanako-local-device-router/tools-cache.json
/opt/hanako-local-device-router/offline-queue.json
/etc/systemd/system/hanako-local-device-router.service
```

Before replacing the Node router, create a root-only timestamped backup of the
application directory and systemd unit. Stage the Rust binary as
`hanako-device-router.new`, validate it with `file`, `ldd`, and `sha256sum`,
then stop the old service and atomically rename the staged binary.

After switching:

```bash
systemctl daemon-reload
systemctl restart hanako-local-device-router
systemctl is-active hanako-local-device-router
curl -fsS http://127.0.0.1:18786/health
```

Rollback restores the saved Node unit and restarts the same service name. Keep
`device-router.cjs`, `devices.json`, `tools-cache.json`, and
`offline-queue.json` until the Rust deployment has completed its observation
period.

The cloud Hana process exposes:

```text
GET  /local-bridge/connect
POST /api/local-bridge/claim
GET  /api/local-bridge/devices
```

Internal forwarding routes must remain loopback-only:

```text
POST /internal/local-bridge/devices/:deviceId/mcp
GET  /internal/local-bridge/devices/:deviceId/health
```

## Reverse Proxy

The reverse proxy serves signed update artifacts directly and preserves WebSocket upgrades for the Hana application:

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

## Verification

Verify all layers:

```text
Public update manifest and ZIP return HTTP 200 without GitHub authentication
Windows /health reports version 1.4.9 and cloud.status=active
Cloud /api/local-bridge/devices reports the device online
Router /health reports version 2.0.0-alpha.2, 34 tools, and all expected devices online
local_device.devices succeeds through the router
local_fs.roots succeeds with an explicit deviceId
local_fs.read_text works through the router
Stopping the Windows service makes the router report offline
Starting the scheduled task restores active/online automatically
```

## Backups

Back up locally:

```text
config.json
data/
logs/
```

Back up on the server:

```text
Hana home data
local-bridge-devices.json
Device Router configuration and offline queue
Cloud Hana source patches and runtime artifacts
```

Never copy one `cloud-identity.json` to two computers that will run
simultaneously. They would share one identity and replace each other's active
connection.

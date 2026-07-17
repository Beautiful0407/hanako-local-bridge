# Hanako Local Bridge Cloud Deployment Guide

This is the repository-safe cloud deployment guide. It intentionally omits
live passwords, access keys, device credentials, and private infrastructure
backups.

## Components

```text
Windows Bridge: 1.2.0
Cloud Hana:     0.401.11 or compatible
Device Router:  0.8.0
```

The Windows bridge actively connects to the cloud:

```text
Windows Local Bridge
  -> ws://<hana-host>/local-bridge/connect
  -> Cloud Hana LocalBridgeGateway
  -> Device Router on 127.0.0.1:18786
  -> local_fs / local_exec / local_device tools
```

Use HTTPS/WSS with a trusted certificate for long-term production use.

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
Bridge version: 1.2.0
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

Deploy `cloud/device-router.cjs` to a root-owned application directory and use
`cloud/hanako-local-device-router.service` as the systemd template.

The router must only listen on:

```text
127.0.0.1:18786
```

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

The reverse proxy must preserve WebSocket upgrades:

```nginx
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
Windows /health reports version 1.2.0 and cloud.status=active
Cloud /api/local-bridge/devices reports the device online
Router /health reports version 0.8.0 and the bridge online
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

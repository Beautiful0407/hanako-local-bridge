# 安全说明

本仓库只包含源码与脱敏后的维护文档。严禁提交运行时的凭据或状态。

以下内容被有意排除在仓库之外：

```text
data/
logs/*.log
build/
release/
CLOUD_HANA_AGENT_MAINTENANCE_MANUAL.md
*.backup-*.md
```

`data/cloud-identity.json` 包含 Windows 桥的私钥与云端设备凭证。它必须只存在于已安装的电脑上，绝不能加入 Git。

`CLOUD_HANA_AGENT_MAINTENANCE_MANUAL.md` 同样必须留在 Git 之外：不得进入发布产物、ZIP 压缩包、安装 EXE 或安装目录。

使用 `config.example.json` 作为配置模板。访问密钥、服务器密码、API 密钥、设备凭证与 SSH 私钥一律不要放进仓库。

发布前检查：

```powershell
rg -n -i "password|token|secret|credential|BEGIN PRIVATE|api[_-]?key" `
  -g "!data/**" -g "!logs/**" -g "!build/**" -g "!release/**" .
```

任何凭据一旦被提交（包括提交到私有仓库），必须立即轮换。只从最新提交中删除是不够的，因为它仍然留在 Git 历史里。

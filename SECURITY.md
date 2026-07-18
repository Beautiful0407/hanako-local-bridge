# Security Notes

This repository contains source code and sanitized maintenance documentation.
Do not commit live credentials or runtime state.

The following content is intentionally excluded:

```text
data/
logs/*.log
build/
release/
CLOUD_HANA_AGENT_MAINTENANCE_MANUAL.md
*.backup-*.md
```

`data/cloud-identity.json` contains the Windows bridge private key and cloud
device credential. It must remain on the installed computer and must never be
added to Git.

`CLOUD_HANA_AGENT_MAINTENANCE_MANUAL.md` must also stay out of Git release
payloads, ZIP archives, Setup EXEs, and installed application directories.

Use `config.example.json` as the configuration template. Keep access keys,
server passwords, API keys, device credentials, and private SSH keys outside
the repository.

Before publishing:

```powershell
rg -n -i "password|token|secret|credential|BEGIN PRIVATE|api[_-]?key" `
  -g "!data/**" -g "!logs/**" -g "!build/**" -g "!release/**" .
```

Rotate a credential immediately if it is ever committed, including to a
private repository. Removing it from the latest commit is not sufficient
because it remains in Git history.

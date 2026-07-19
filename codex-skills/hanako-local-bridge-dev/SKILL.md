---
name: hanako-local-bridge-dev
description: "开发和维护 Beautiful0407/hanako--MCP- 的 Hanako Local Bridge MCP。用于继续开发本地文件读写、图片读取、PowerShell/Python 执行、Windows Rust Bridge、WebView2 托盘管理器、签名更新器、内嵌安装器、云端 WebSocket、Linux Device Router、多设备路由、自动恢复和在线更新；也用于诊断本地 MCP 不可用、窗口弹出、托盘异常、设备离线、更新失败、安装覆盖失败，执行测试、打包、GitHub Release、VPS Alpha feed 发布、回滚和开发文档维护。触发词包括：继续开发文件桥、修改 MCP、修复 Hanako Local Bridge、增加本地工具、打包安装器、发布新版、在线更新、云端设备路由、维护开发手册。"
---

# Hanako Local Bridge Development

把私人仓库视为源码和文档的唯一事实来源。先验证当前状态，再修改；完成代码、测试、
发布和文档闭环后再结束。

## Source Of Truth

默认仓库：

```text
C:\Users\30456\Documents\hanako 开发\hanako-mcp-publish-20260717
```

仓库源码中的 Skill：

```text
codex-skills\hanako-local-bridge-dev
```

本机安装副本：

```text
C:\Users\30456\.codex\skills\hanako-local-bridge-dev
```

不要把本 Skill 放进 HanaAgent 的 `skills2set/`，也不要上传到 VPS 的
`/root/.hanako/skills/`。它只供 Codex 开发工作使用。

## Hard Diagnostic Rules

### Online Update Timeout

管理器显示在线安装超时时，禁止仅凭端口、PID、UI 提示或泛化日志判断，也不要重复点击。
旧 Bridge 退出会主动切断原 HTTP 连接。必须读取正式安装目录中的固定证据：

```text
%LOCALAPPDATA%\HanakoLocalBridge\data\update-state.json
%LOCALAPPDATA%\HanakoLocalBridge\payload-manifest.json
%LOCALAPPDATA%\HanakoLocalBridge\logs\update.log
http://127.0.0.1:8787/health
http://127.0.0.1:8788/health
```

用升级前备份核对 `config.json` SHA256。只有以下条件同时成立才确认成功：

```text
update-state.status = succeeded
installedVersion = 目标版本
payload-manifest.version = 目标版本
8787 和 8788 均 ok=true
8787 cloud.status = active
config.json SHA256 未变化
```

`status=failed` 或明确回滚才确认失败；worker 仍运行或状态未终结时继续等待。

## Start Every Task

1. 若仓库存在匹配的 `.planning/.active-plan`，先使用 `$plan-durably` 恢复并核对实际
   Git、文件和运行时状态。
2. 运行：

   ```powershell
   powershell.exe -NoProfile -ExecutionPolicy Bypass -File `
     codex-skills\hanako-local-bridge-dev\scripts\inspect-environment.ps1 `
     -RepoRoot .
   ```

3. 检查 `git status --short --branch`，保留用户已有改动，不得回退无关文件。
4. 读取 [repo-snapshot.md](references/repo-snapshot.md)。若版本、标签或安装状态已经
   漂移，先运行 `refresh-repo-snapshot.ps1`。
5. 修改代码前使用 CodeGraph 定位入口、调用关系和影响范围；索引缺少刚写入的文件时，
   再用 `rg` 和精确文件读取补充。

## Choose The Workflow

- **功能、重构、工具面变化**：读取
  [development-manual.md](references/development-manual.md) 和
  [architecture.md](references/architecture.md)。
- **错误、崩溃、回归、截图问题**：同时使用 `$hunt`；先写出可证伪的根因句和回归检查。
- **管理器 UI、托盘、WebView2、窗口行为**：同时使用 `$ui` 或 `$hunt`，并执行真实
  Windows 运行时验证，不能只编译。
- **安装器、在线更新、版本、GitHub Release、VPS feed、回滚**：读取
  [release-runbook.md](references/release-runbook.md) 并按顺序执行。
- **Cloud Hana 主程序源码升级**：不要由本 Skill 处理，改用 `$cloud-hanako-upgrade`。
- **Cloud Hana 日常生产故障**：改用 `$cloud-hanako-ops`。本 Skill 只负责 Local
  Bridge 和 Device Router 自身的兼容开发与独立 release feed。

## Engineering Rules

- 优先维护 Rust 实现：`apps/` 和 `crates/`。Legacy Node/PowerShell/WinUI 代码仍是
  稳定版兼容和迁移基线，不要无关删除。
- 保持 MCP、文件格式、配置、设备身份和云端协议向后兼容。新增字段使用可选默认值，
  未知配置字段必须保留。
- 安装、覆盖和在线更新必须保留：

  ```text
  config.json
  data\
  logs\
  未知用户文件
  ```

- Release Bridge 和 Manager 必须是 Windows GUI 子系统；Debug Bridge 保留控制台。
- 后台任务必须无窗口、单实例、登录启动、周期自愈。关闭 Manager 不能停止 Bridge。
- 本地接口只监听 loopback。不要把 approval token、设备私钥、claim token、登录密钥、
  签名私钥或完整生产日志写入 Git、诊断输出或开发手册。
- 不要手工把 `target\release` 中的 EXE 覆盖到正式安装目录。使用内嵌安装器或签名更新。
- PowerShell 脚本使用 `powershell.exe -ExecutionPolicy Bypass -File`；Node 命令优先
  使用 `npm.cmd`/`npx.cmd`，避免本机执行策略拦截 `.ps1` shim。

## Change Workflow

1. 定义用户可观察的成功条件和最小回归测试。
2. 对缺陷先让测试在旧行为上失败；对新功能先覆盖核心契约和兼容边界。
3. 只修改所属模块，避免顺带迁移或清理 Legacy 代码。
4. 按风险运行分层测试；发布前执行完整质量门。
5. 需要提交产品开发变更时更新产品版本、`CHANGELOG.md` 和相关手册。纯 Skill 文档
   维护由 Git 提交追踪，不伪造产品 release。
6. 生成 release 后核对版本、大小、SHA256、签名清单和安装器内嵌 payload。
7. 修改 VPS feed 前备份旧 manifest，先上传版本化文件，最后原子替换 current
   manifest；稳定和 Alpha feed 必须隔离。
8. 在真实安装目录完成升级、配置哈希、双 health、云端状态、窗口句柄和自愈验证。
9. 更新 Skill 手册与动态快照，提交并推送仓库。

## Validation

完整命令和选择矩阵见
[development-manual.md](references/development-manual.md)。最低完成标准：

- `cargo fmt`, Clippy 和相关 Rust tests 通过。
- 受影响的 Node/PowerShell 集成测试通过。
- UI/native 变更完成真实运行验证。
- 安装/update 变更通过对应 smoke test。
- 发布变更验证本机安装和公开 feed，且稳定 feed 未被误改。
- `git diff --check` 通过，工作树只包含预期文件。

## Maintain This Skill

产品结构、约束或发布流程变化后：

1. 更新对应 `references/*.md`，不要把大段细节塞回 `SKILL.md`。
2. 运行：

   ```powershell
   powershell.exe -NoProfile -ExecutionPolicy Bypass -File `
     codex-skills\hanako-local-bridge-dev\scripts\refresh-repo-snapshot.ps1 `
     -RepoRoot .
   ```

3. 验证 Skill：

   ```powershell
   python "$env:USERPROFILE\.codex\skills\.system\skill-creator\scripts\quick_validate.py" `
     codex-skills\hanako-local-bridge-dev
   ```

4. 同步安装副本：

   ```powershell
   powershell.exe -NoProfile -ExecutionPolicy Bypass -File `
     codex-skills\hanako-local-bridge-dev\scripts\sync-installed-skill.ps1
   ```

5. 重新启动 Codex，使新建 Skill 或元数据变化在后续任务中生效。

## References

- [architecture.md](references/architecture.md)：模块、数据、协议和所有权边界。
- [development-manual.md](references/development-manual.md)：日常开发、调试、测试和维护。
- [release-runbook.md](references/release-runbook.md)：签名构建、发布、VPS、升级和回滚。
- [repo-snapshot.md](references/repo-snapshot.md)：脚本生成的当前版本、提交和运行时快照。

const http = require("http");

function escapeHtml(value) {
  return String(value ?? "")
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#39;");
}

function readBody(req) {
  return new Promise((resolve, reject) => {
    const chunks = [];
    req.on("data", (chunk) => chunks.push(chunk));
    req.on("end", () => resolve(Buffer.concat(chunks).toString("utf8")));
    req.on("error", reject);
  });
}

function json(res, status, data, headers = {}) {
  res.writeHead(status, {
    "Content-Type": "application/json; charset=utf-8",
    "Cache-Control": "no-store",
    "X-Content-Type-Options": "nosniff",
    ...headers,
  });
  res.end(JSON.stringify(data));
}

function clientIdentityHeaders(req, allowedBrowserHosts) {
  const origin = String(req.headers.origin || "").trim();
  if (!origin) return {};
  let hostname = "";
  try {
    hostname = new URL(origin).hostname.toLowerCase();
  } catch {
    return null;
  }
  const allowed = new Set(
    (allowedBrowserHosts || [])
      .map((item) => String(item || "").trim().toLowerCase())
      .filter(Boolean),
  );
  if (!allowed.has(hostname)) return null;
  return {
    "Access-Control-Allow-Origin": origin,
    "Access-Control-Allow-Methods": "GET, OPTIONS",
    "Access-Control-Allow-Headers": "Content-Type",
    "Access-Control-Allow-Private-Network": "true",
    "Vary": "Origin, Access-Control-Request-Private-Network",
  };
}

function renderPage(access, execution, port) {
  const token = JSON.stringify(access.approvalToken);
  const fullTrustNotice = access.fullTrust
    ? `<div class="notice" style="border-left-color:#26834a;background:#edf9f1">
      <strong>Full trust mode is enabled.</strong>
      File read/write and PowerShell/Python execution are approved automatically. This page is status-only.
    </div>`
    : "";
  return `<!doctype html>
<html lang="zh-CN">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <meta http-equiv="Content-Security-Policy" content="default-src 'self'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; connect-src 'self'">
  <title>Hanako 本地文件授权</title>
  <style>
    :root { color-scheme: light; font-family: "Segoe UI", "Microsoft YaHei", sans-serif; }
    body { margin: 0; background: #f5f6f7; color: #1d2329; }
    main { max-width: 980px; margin: 32px auto; padding: 0 20px 40px; }
    h1 { font-size: 26px; margin: 0 0 8px; }
    h2 { font-size: 18px; margin: 28px 0 10px; }
    p { line-height: 1.65; }
    .notice { border-left: 4px solid #c68022; background: #fff7e8; padding: 12px 14px; }
    .panel { background: #fff; border: 1px solid #d8dde3; border-radius: 6px; overflow: hidden; }
    .item { padding: 14px 16px; border-top: 1px solid #e8ebee; }
    .item:first-child { border-top: 0; }
    .path { font-family: Consolas, monospace; word-break: break-all; margin: 5px 0; }
    .meta { color: #5d6670; font-size: 13px; }
    .actions { display: flex; flex-wrap: wrap; gap: 8px; margin-top: 10px; }
    button, select, input { font: inherit; }
    button { border: 1px solid #aab2bb; background: #fff; padding: 7px 11px; border-radius: 4px; cursor: pointer; }
    button.primary { background: #1769aa; border-color: #1769aa; color: white; }
    button.danger { color: #a12622; }
    select, input { border: 1px solid #aab2bb; padding: 7px; border-radius: 4px; }
    .empty { padding: 18px; color: #66707a; }
    .badge { display: inline-block; padding: 2px 7px; border-radius: 10px; background: #edf1f4; font-size: 12px; }
    #message { min-height: 22px; margin: 12px 0; color: #1769aa; }
  </style>
</head>
<body>
<main>
  ${fullTrustNotice}
  <h1>Hanako 本地文件授权</h1>
  <p>此页面只监听 <code>127.0.0.1:${port}</code>，不会通过 SSH 隧道暴露给云端。</p>
  <div class="notice">
    云端 Agent 只能提交访问请求，不能自行批准。批准 <strong>读写</strong> 后，它可以创建、覆盖、移动文件，
    并把文件移动到该授权目录的 <code>.hana-trash</code>。
  </div>
  <div id="message"></div>

  <h2>待审批请求</h2>
  <section class="panel" id="pending"></section>

  <h2>已授权目录</h2>
  <section class="panel" id="grants"></section>

  <h2>待审批脚本执行</h2>
  <section class="panel" id="execution-pending"></section>

  <h2>已授权脚本执行</h2>
  <section class="panel" id="execution-authorizations"></section>
</main>
<script>
const TOKEN = ${token};
const headers = { "Content-Type": "application/json", "X-Approval-Token": TOKEN };

function esc(value) {
  return String(value ?? "").replace(/[&<>"']/g, (ch) => ({
    "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;"
  }[ch]));
}

async function api(path, options = {}) {
  const res = await fetch(path, { ...options, headers: { ...headers, ...(options.headers || {}) } });
  const data = await res.json();
  if (!res.ok) throw new Error(data.error || "request failed");
  return data;
}

async function refresh() {
  const state = await api("/api/state");
  const pending = state.requests.filter((item) => item.status === "pending");
  document.querySelector("#pending").innerHTML = pending.length ? pending.map((item) => \`
    <div class="item">
      <div><span class="badge">\${esc(item.mode)}</span></div>
      <div class="path">\${esc(item.path)}</div>
      <div class="meta">原因：\${esc(item.reason || "未填写")} · \${esc(item.createdAt)}</div>
      <div class="actions">
        <input id="name-\${item.id}" value="\${esc(item.name || "")}" placeholder="local:// 别名">
        <select id="mode-\${item.id}">
          <option value="read" \${item.mode === "read" ? "selected" : ""}>只读</option>
          <option value="read_write" \${item.mode === "read_write" ? "selected" : ""}>读写</option>
        </select>
        <button class="primary" onclick="approve('\${item.id}')">批准</button>
        <button class="danger" onclick="deny('\${item.id}')">拒绝</button>
      </div>
    </div>
  \`).join("") : '<div class="empty">没有待审批请求。</div>';

  document.querySelector("#grants").innerHTML = state.grants.length ? state.grants.map((item) => \`
    <div class="item">
      <div><strong>local://\${esc(item.id)}</strong> <span class="badge">\${esc(item.mode)}</span></div>
      <div class="path">\${esc(item.path)}</div>
      <div class="meta">来源：\${esc(item.source)}</div>
      \${item.source === "bootstrap" ? "" : \`<div class="actions"><button class="danger" onclick="revoke('\${item.id}')">撤销授权</button></div>\`}
    </div>
  \`).join("") : '<div class="empty">没有已授权目录。</div>';

  const executionPending = state.executionRequests.filter((item) => item.status === "pending");
  document.querySelector("#execution-pending").innerHTML = executionPending.length ? executionPending.map((item) => \`
    <div class="item">
      <div><strong>\${esc(item.runtime)}</strong> <span class="badge">SHA256 锁定</span></div>
      <div class="path">\${esc(item.scriptPath)}</div>
      <div class="meta">参数：\${esc(JSON.stringify(item.arguments || []))}</div>
      <div class="meta">工作目录：\${esc(item.cwd)} · 超时：\${esc(item.timeoutSeconds)} 秒</div>
      <div class="meta">原因：\${esc(item.reason || "未填写")} · \${esc(item.createdAt)}</div>
      <div class="actions">
        <button class="primary" onclick="approveExecution('\${item.id}', 'once')">批准一次</button>
        <button onclick="approveExecution('\${item.id}', 'trusted')">始终信任当前哈希和参数</button>
        <button class="danger" onclick="denyExecution('\${item.id}')">拒绝</button>
      </div>
    </div>
  \`).join("") : '<div class="empty">没有待审批脚本。</div>';

  document.querySelector("#execution-authorizations").innerHTML = state.executionAuthorizations.length
    ? state.executionAuthorizations.map((item) => \`
      <div class="item">
        <div><strong>\${esc(item.runtime)}</strong> <span class="badge">\${esc(item.scope)}</span></div>
        <div class="path">\${esc(item.scriptPath)}</div>
        <div class="meta">参数：\${esc(JSON.stringify(item.arguments || []))}</div>
        <div class="meta">SHA256：\${esc(item.scriptSha256)}</div>
        <div class="meta">来源：\${esc(item.source)} · 剩余次数：\${esc(item.usesRemaining ?? "不限")}</div>
        <div class="actions">
          <button class="danger" onclick="revokeExecution('\${item.id}')">撤销执行授权</button>
        </div>
      </div>
    \`).join("")
    : '<div class="empty">没有已授权脚本。</div>';
}

async function act(fn) {
  try {
    await fn();
    document.querySelector("#message").textContent = "操作成功。";
    await refresh();
  } catch (err) {
    document.querySelector("#message").textContent = err.message;
  }
}

function approve(id) {
  return act(() => api("/api/requests/" + id + "/approve", {
    method: "POST",
    body: JSON.stringify({
      mode: document.querySelector("#mode-" + id).value,
      name: document.querySelector("#name-" + id).value
    })
  }));
}
function deny(id) {
  return act(() => api("/api/requests/" + id + "/deny", { method: "POST", body: "{}" }));
}
function revoke(id) {
  if (!confirm("确定撤销 local://" + id + " 的授权吗？")) return;
  return act(() => api("/api/grants/" + id + "/revoke", { method: "POST", body: "{}" }));
}
function approveExecution(id, scope) {
  return act(() => api("/api/execution/requests/" + id + "/approve", {
    method: "POST",
    body: JSON.stringify({ scope })
  }));
}
function denyExecution(id) {
  return act(() => api("/api/execution/requests/" + id + "/deny", { method: "POST", body: "{}" }));
}
function revokeExecution(id) {
  if (!confirm("确定撤销这个脚本执行授权吗？")) return;
  return act(() => api("/api/execution/authorizations/" + id + "/revoke", {
    method: "POST",
    body: "{}"
  }));
}

refresh().catch((err) => {
  document.querySelector("#message").textContent = err.message;
});
setInterval(() => refresh().catch(() => {}), 5000);
</script>
</body>
</html>`;
}

function validateLocalHost(req) {
  const host = String(req.headers.host || "").split(":")[0].toLowerCase();
  return host === "127.0.0.1" || host === "localhost" || host === "[::1]";
}

function authorized(req, access) {
  return req.headers["x-approval-token"] === access.approvalToken;
}

function createApprovalServer(options) {
  const access = options.access;
  const execution = options.execution;
  const host = options.host;
  const port = options.port;
  const device = options.device || null;
  const version = options.version || null;
  const allowedBrowserHosts = options.allowedBrowserHosts || [];
  const cloudIdentity = typeof options.cloudIdentity === "function"
    ? options.cloudIdentity
    : () => null;

  return http.createServer(async (req, res) => {
    try {
      if (!validateLocalHost(req)) return json(res, 403, { error: "localhost only" });
      const url = new URL(req.url, `http://${req.headers.host || `${host}:${port}`}`);

      if (req.method === "GET" && url.pathname === "/") {
        res.writeHead(200, {
          "Content-Type": "text/html; charset=utf-8",
          "Cache-Control": "no-store",
          "X-Frame-Options": "DENY",
          "X-Content-Type-Options": "nosniff",
        });
        return res.end(renderPage(access, execution, port));
      }
      if (req.method === "GET" && url.pathname === "/health") {
        return json(res, 200, {
          ok: true,
          trustMode: access.fullTrust ? "full" : "approval",
          approvalRequired: !access.fullTrust,
          pending: access.listRequests().filter((item) => item.status === "pending").length,
          pendingExecutions: execution.listRequests().filter((item) => item.status === "pending").length,
        });
      }
      if (url.pathname === "/api/client-identity") {
        const headers = clientIdentityHeaders(req, allowedBrowserHosts);
        if (!headers) return json(res, 403, { error: "origin not allowed" });
        if (req.method === "OPTIONS") return json(res, 200, { ok: true }, headers);
        if (req.method === "GET") {
          return json(res, 200, {
            ok: true,
            version,
            device,
            cloud: cloudIdentity(),
          }, headers);
        }
      }
      if (!authorized(req, access)) return json(res, 403, { error: "invalid approval token" });
      if (req.method === "GET" && url.pathname === "/api/state") {
        return json(res, 200, {
          trustMode: access.fullTrust ? "full" : "approval",
          approvalRequired: !access.fullTrust,
          grants: access.listGrants(),
          requests: access.listRequests(),
          executionAuthorizations: execution.listAuthorizations(),
          executionRequests: execution.listRequests(),
        });
      }
      const approve = url.pathname.match(/^\/api\/requests\/([^/]+)\/approve$/);
      if (req.method === "POST" && approve) {
        const body = JSON.parse((await readBody(req)) || "{}");
        return json(res, 200, { grant: await access.approveRequest(decodeURIComponent(approve[1]), body) });
      }
      const deny = url.pathname.match(/^\/api\/requests\/([^/]+)\/deny$/);
      if (req.method === "POST" && deny) {
        return json(res, 200, { request: await access.denyRequest(decodeURIComponent(deny[1])) });
      }
      const revoke = url.pathname.match(/^\/api\/grants\/([^/]+)\/revoke$/);
      if (req.method === "POST" && revoke) {
        return json(res, 200, { grant: await access.revokeGrant(decodeURIComponent(revoke[1])) });
      }
      const approveExecution = url.pathname.match(/^\/api\/execution\/requests\/([^/]+)\/approve$/);
      if (req.method === "POST" && approveExecution) {
        const body = JSON.parse((await readBody(req)) || "{}");
        return json(res, 200, {
          authorization: await execution.approveRequest(decodeURIComponent(approveExecution[1]), body),
        });
      }
      const denyExecution = url.pathname.match(/^\/api\/execution\/requests\/([^/]+)\/deny$/);
      if (req.method === "POST" && denyExecution) {
        return json(res, 200, {
          request: await execution.denyRequest(decodeURIComponent(denyExecution[1])),
        });
      }
      const revokeExecution = url.pathname.match(/^\/api\/execution\/authorizations\/([^/]+)\/revoke$/);
      if (req.method === "POST" && revokeExecution) {
        return json(res, 200, {
          authorization: await execution.revokeAuthorization(decodeURIComponent(revokeExecution[1])),
        });
      }
      return json(res, 404, { error: "not found" });
    } catch (err) {
      return json(res, 500, { error: err.message || String(err), code: err.code || "approval_error" });
    }
  });
}

module.exports = {
  createApprovalServer,
  escapeHtml,
};

use std::{
    collections::{HashMap, HashSet, VecDeque},
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant, SystemTime},
};

use base64::Engine;
use futures_util::future::BoxFuture;
use globset::{Glob, GlobMatcher};
use hanako_bridge_core::{
    BridgeError,
    path::{AccessMode, ResolvedPath, public_path},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;
use uuid::Uuid;
use walkdir::WalkDir;

use crate::state::{AppState, FileFingerprint, WatchRecord};

fn content_text(text: impl Into<String>) -> Value {
    json!({ "content": [{ "type": "text", "text": text.into() }] })
}

fn content_json(value: Value) -> Value {
    content_text(serde_json::to_string_pretty(&value).unwrap_or_else(|_| "{}".to_string()))
}

fn rpc_result(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn rpc_error(id: Value, error: &BridgeError) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": -32000,
            "message": error.to_string(),
            "data": {
                "code": error.code(),
                "expected": error.expected(),
                "actual": error.actual()
            }
        }
    })
}

fn tool(name: &str, title: &str, description: &str, properties: Value, required: &[&str]) -> Value {
    json!({
        "name": name,
        "title": title,
        "description": description,
        "inputSchema": {
            "type": "object",
            "properties": properties,
            "required": required
        }
    })
}

pub fn tool_definitions(state: &AppState) -> Vec<Value> {
    let path_property = json!({
        "path": {
            "type": "string",
            "description": if state.full_trust {
                "A local:// alias, device:// path, or absolute Windows drive path."
            } else {
                "A local:// alias or device:// path inside an authorized root."
            }
        }
    });
    let mut defs = vec![
        tool(
            "local_fs.roots",
            "List local roots",
            "List the local Windows roots available to Hana Agent.",
            json!({}),
            &[],
        ),
        tool(
            "local_fs.request_access",
            "Request local path access",
            if state.full_trust {
                "Return immediate authorization because full trust is enabled."
            } else {
                "Request access to one absolute Windows directory."
            },
            if state.full_trust {
                json!({
                    "path": { "type": "string" },
                    "mode": { "type": "string", "enum": ["read", "read_write"] },
                    "reason": { "type": "string" }
                })
            } else {
                json!({
                    "path": { "type": "string" },
                    "mode": { "type": "string", "enum": ["read", "read_write"] },
                    "reason": { "type": "string" },
                    "userAuthorizationQuote": { "type": "string" }
                })
            },
            &["path"],
        ),
        tool(
            "local_fs.access_status",
            "Check local path access",
            "Check the current authorization state for a local access request.",
            json!({ "requestId": { "type": "string" } }),
            &["requestId"],
        ),
        tool(
            "local_fs.list",
            "List a local directory",
            "List files and directories on the connected Windows computer.",
            json!({
                "path": path_property["path"],
                "cursor": { "type": "string" },
                "limit": { "type": "number" }
            }),
            &["path"],
        ),
        tool(
            "local_fs.stat",
            "Inspect a local path",
            "Return metadata for one file or directory.",
            path_property.clone(),
            &["path"],
        ),
        tool(
            "local_fs.hash",
            "Hash a local file",
            "Return SHA-256 for one local file.",
            path_property.clone(),
            &["path"],
        ),
        tool(
            "local_fs.read_text",
            "Read a local text file",
            "Read a UTF-8, UTF-16LE, or UTF-16BE local text file.",
            path_property.clone(),
            &["path"],
        ),
        tool(
            "local_fs.read_lines",
            "Read local text lines",
            "Read a bounded line range while preserving encoding metadata.",
            json!({
                "path": path_property["path"],
                "startLine": { "type": "number" },
                "lineCount": { "type": "number" }
            }),
            &["path"],
        ),
        tool(
            "local_fs.read_chunk",
            "Read a local file chunk",
            "Read a bounded byte range as base64.",
            json!({
                "path": path_property["path"],
                "offset": { "type": "number" },
                "length": { "type": "number" }
            }),
            &["path"],
        ),
        tool(
            "local_fs.read_image",
            "Read a local image",
            "Return PNG, JPEG, GIF, or WebP image data to the model.",
            path_property.clone(),
            &["path"],
        ),
        tool(
            "local_fs.search",
            "Search local files",
            "Search a bounded local directory tree by name and optional glob. When content or contentRegex is provided, also match file contents (text files only, BOM-aware). Returns contentMatches with line numbers for hits.",
            json!({
                "path": path_property["path"],
                "query": { "type": "string" },
                "glob": { "type": "string" },
                "exclude": { "type": "array", "items": { "type": "string" } },
                "maxDepth": { "type": "number" },
                "limit": { "type": "number" },
                "timeoutMs": { "type": "number" },
                "maxVisited": { "type": "number" },
                "content": { "type": "string" },
                "contentRegex": { "type": "string" },
                "contentMaxBytes": { "type": "number" }
            }),
            &["path"],
        ),
        tool(
            "local_fs.watch",
            "Watch a local path",
            "Start a bounded file-change watch on one file or directory.",
            json!({
                "path": path_property["path"],
                "recursive": { "type": "boolean" },
                "debounceMs": { "type": "number" }
            }),
            &["path"],
        ),
        tool(
            "local_fs.watch_events",
            "Read local file changes",
            "Read or wait for events from a local file watch.",
            json!({
                "watchId": { "type": "string" },
                "afterSequence": { "type": "number" },
                "limit": { "type": "number" },
                "waitMs": { "type": "number" }
            }),
            &["watchId"],
        ),
        tool(
            "local_fs.unwatch",
            "Stop a local file watch",
            "Stop and remove one local file watch.",
            json!({ "watchId": { "type": "string" } }),
            &["watchId"],
        ),
        tool(
            "local_fs.write_text",
            "Write local text",
            "Create or atomically replace a text file with optional SHA-256 concurrency protection.",
            json!({
                "path": path_property["path"],
                "text": { "type": "string" },
                "overwrite": { "type": "boolean" },
                "expectedSha256": { "type": "string" },
                "createParents": { "type": "boolean" },
                "encoding": { "type": "string", "enum": ["utf8", "utf16le", "utf16be"] },
                "bom": { "type": "boolean" }
            }),
            &["path", "text"],
        ),
        tool(
            "local_fs.append_text",
            "Append local text",
            "Atomically append text while preserving the file encoding and BOM.",
            json!({
                "path": path_property["path"],
                "text": { "type": "string" },
                "expectedSha256": { "type": "string" },
                "createParents": { "type": "boolean" },
                "encoding": { "type": "string", "enum": ["utf8", "utf16le", "utf16be"] },
                "bom": { "type": "boolean" }
            }),
            &["path", "text"],
        ),
        tool(
            "local_fs.apply_patch",
            "Apply exact local text edits",
            "Apply exact text replacements to a SHA-256 locked file.",
            json!({
                "path": path_property["path"],
                "expectedSha256": { "type": "string" },
                "edits": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "oldText": { "type": "string" },
                            "newText": { "type": "string" },
                            "expectedOccurrences": { "type": "number" }
                        },
                        "required": ["oldText", "newText"]
                    }
                }
            }),
            &["path", "expectedSha256", "edits"],
        ),
        tool(
            "local_fs.write_base64",
            "Write local binary data",
            "Create or atomically replace a local file from base64 data.",
            json!({
                "path": path_property["path"],
                "dataBase64": { "type": "string" },
                "overwrite": { "type": "boolean" },
                "expectedSha256": { "type": "string" },
                "createParents": { "type": "boolean" }
            }),
            &["path", "dataBase64"],
        ),
        tool(
            "local_fs.append_base64",
            "Append local binary data",
            "Append base64-decoded bytes to a local file, creating it if missing. For large transfers, split the payload into multiple sequential calls; each call appends its chunk to the same path. Pass finalSha256 on the last chunk to verify the complete file, or expectedSha256 to verify the file as it exists before this append.",
            json!({
                "path": path_property["path"],
                "dataBase64": { "type": "string" },
                "expectedSha256": { "type": "string" },
                "finalSha256": { "type": "string" },
                "createParents": { "type": "boolean" }
            }),
            &["path", "dataBase64"],
        ),
        tool(
            "local_fs.mkdir",
            "Create a local directory",
            "Create a local directory and optionally its parents.",
            json!({
                "path": path_property["path"],
                "recursive": { "type": "boolean" }
            }),
            &["path"],
        ),
        tool(
            "local_fs.copy",
            "Copy a local path",
            "Copy a file or directory to a new local path.",
            json!({
                "source": { "type": "string" },
                "destination": { "type": "string" },
                "createParents": { "type": "boolean" }
            }),
            &["source", "destination"],
        ),
        tool(
            "local_fs.move",
            "Move a local path",
            "Move a file or directory to a new local path.",
            json!({
                "source": { "type": "string" },
                "destination": { "type": "string" },
                "createParents": { "type": "boolean" }
            }),
            &["source", "destination"],
        ),
        tool(
            "local_fs.delete_to_trash",
            "Move a local path to bridge trash",
            "Move a file or directory into a recoverable .hana-trash directory.",
            path_property,
            &["path"],
        ),
        execution_tool(
            state,
            "local_exec.runtimes",
            "Detect local PowerShell and Python",
            json!({ "refresh": { "type": "boolean" } }),
            &[],
        ),
        execution_tool(
            state,
            "local_exec.request_run",
            "Request an exact local script execution",
            execution_properties(state),
            &["runtime", "scriptPath"],
        ),
        execution_tool(
            state,
            "local_exec.execute",
            "Execute one local script and wait",
            execution_properties(state),
            &["runtime", "scriptPath"],
        ),
        execution_tool(
            state,
            "local_exec.request_status",
            "Check a local execution request",
            json!({ "requestId": { "type": "string" } }),
            &["requestId"],
        ),
        execution_tool(
            state,
            "local_exec.authorizations",
            "List local execution authorizations",
            json!({}),
            &[],
        ),
        execution_tool(
            state,
            "local_exec.run",
            "Start an approved local script",
            json!({ "authorizationId": { "type": "string" } }),
            &["authorizationId"],
        ),
        execution_tool(
            state,
            "local_exec.job_status",
            "Check local script job status",
            json!({ "jobId": { "type": "string" } }),
            &["jobId"],
        ),
        execution_tool(
            state,
            "local_exec.job_output",
            "Read local script job output",
            json!({
                "jobId": { "type": "string" },
                "maxChars": { "type": "number" }
            }),
            &["jobId"],
        ),
        execution_tool(
            state,
            "local_exec.cancel_job",
            "Cancel a local script job",
            json!({ "jobId": { "type": "string" } }),
            &["jobId"],
        ),
        tool(
            "local_exec.list_processes",
            "List running processes",
            "List running processes on the connected Windows computer, optionally filtered by an image-name substring (case-insensitive). Returns pid, name, and sessionId for each. Use this to see how many instances of an application are running before terminating any.",
            json!({
                "name": { "type": "string" },
                "limit": { "type": "number" }
            }),
            &[],
        ),
        tool(
            "local_exec.terminate",
            "Terminate a process or process tree",
            "Terminate a process by pid, or every process whose image name contains a substring. By default kills the whole process tree (tree=false for a single process). Returns which pids were terminated, which failed and why, and which were protected. This bridge, its running job workers, and the Hanako manager/updater are always protected and never killed. NOTE: this is not a security boundary — it only prevents the bridge from harming itself. A by-name match of more than one process requires confirm:true (or a specific pid) so a vague instruction cannot mass-kill an application family.",
            json!({
                "pid": { "type": "number" },
                "name": { "type": "string" },
                "tree": { "type": "boolean" },
                "confirm": { "type": "boolean" }
            }),
            &[],
        ),
    ];
    // ── Nuphus 桌面/浏览器自动化(能力授权)──
    defs.push(tool(
        "nuphus.request_access",
        "Request nuphus automation capability",
        "Request a capability grant for nuphus desktop/browser automation tools. The quote must be the exact current user authorization message mentioning the capability domain (e.g. 允许操作桌面/鼠标/浏览器/剪贴板/窗口). Write tools (mouse/keyboard/window control/clipboard write/browser control) require an active grant; read-only tools (screenshots, lists) run without one.",
        json!({
            "capability": { "type": "string", "enum": ["desktop.control", "desktop.input", "desktop.window", "desktop.clipboard", "browser.control"] },
            "quote": { "type": "string" }
        }),
        &["capability", "quote"],
    ));
    defs.push(tool(
        "nuphus.access_status",
        "List nuphus automation grants",
        "List all nuphus desktop/browser automation capability grants with their ids, capability, source and expiry.",
        json!({}),
        &[],
    ));
    defs.push(tool(
        "nuphus.revoke",
        "Revoke a nuphus automation grant",
        "Revoke a nuphus capability grant by id (see nuphus.access_status).",
        json!({ "grantId": { "type": "string" } }),
        &["grantId"],
    ));
    for nuphus_tool in nuphus_mcp_core::tools::all_tools() {
        defs.push(json!({
            "name": nuphus_tool.name,
            "title": nuphus_tool.name,
            "description": nuphus_tool.description,
            "inputSchema": nuphus_tool.input_schema,
        }));
    }
    defs
}

fn execution_properties(state: &AppState) -> Value {
    let mut properties = json!({
        "runtime": { "type": "string", "enum": ["powershell", "python"] },
        "scriptPath": { "type": "string" },
        "arguments": { "type": "array", "items": { "type": "string" } },
        "cwd": { "type": "string" },
        "timeoutSeconds": { "type": "number" },
        "reason": { "type": "string" }
    });
    if !state.full_trust {
        properties["userAuthorizationQuote"] = json!({ "type": "string" });
    }
    properties
}

fn execution_tool(
    state: &AppState,
    name: &str,
    title: &str,
    properties: Value,
    required: &[&str],
) -> Value {
    let description = if state.full_trust && name == "local_exec.execute" {
        "Execute an absolute .ps1 or .py script immediately without a quote or approval, wait for completion, and return stdout, stderr, and exit status. The execution remains SHA-256 locked and audited. This call blocks until the script finishes, so it is only for short tasks: anything that may run longer than ~30s (npm install, large recursive directory scans, downloads) should instead use request_run + run to start the job and poll job_status/job_output, which returns immediately and is not bound by the request timeout. To capture output from a child process the script starts, run it inline (& the command, or Start-Process with -NoNewWindow -Wait); a bare Start-Process detaches and its stdout is not captured."
    } else if name == "local_exec.execute" {
        "Validate one exact PowerShell or Python task, obtain chat or local authorization, execute it, wait for completion, and return stdout, stderr, and exit status. This call blocks until the script finishes, so it is only for short tasks: anything that may run longer than ~30s (npm install, large recursive directory scans, downloads) should instead use request_run + run and poll job_status/job_output, which returns immediately and is not bound by the request timeout. To capture output from a child process the script starts, run it inline (& the command, or Start-Process with -NoNewWindow -Wait); a bare Start-Process detaches and its stdout is not captured."
    } else {
        "Execute and inspect an exact PowerShell or Python task on the connected Windows computer."
    };
    tool(name, title, description, properties, required)
}

pub async fn handle_payload(state: Arc<AppState>, payload: Value) -> Value {
    if let Some(messages) = payload.as_array() {
        let mut responses = Vec::with_capacity(messages.len());
        for message in messages {
            if let Some(response) = handle_message(Arc::clone(&state), message.clone()).await {
                responses.push(response);
            }
        }
        return Value::Array(responses);
    }
    handle_message(state, payload).await.unwrap_or(Value::Null)
}

async fn handle_message(state: Arc<AppState>, message: Value) -> Option<Value> {
    let id = message.get("id").cloned().unwrap_or(Value::Null);
    let method = message
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if method == "notifications/initialized" {
        return None;
    }
    let result = match method {
        "initialize" => Ok(json!({
            "protocolVersion": message
                .pointer("/params/protocolVersion")
                .and_then(Value::as_str)
                .unwrap_or("2025-06-18"),
            "capabilities": { "tools": { "listChanged": true } },
            "serverInfo": {
                "name": "hana-local-fs-mcp",
                "title": "Hanako Local File Bridge",
                "version": env!("CARGO_PKG_VERSION")
            }
        })),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tool_definitions(&state) })),
        "tools/call" => {
            let name = message
                .pointer("/params/name")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let arguments = message
                .pointer("/params/arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            let started = Instant::now();
            state.record_tool_call(name);
            let result = call_tool(&state, name, &arguments).await;
            let audit = match &result {
                Ok(_) => json!({
                    "kind": "mcp_tool_call",
                    "tool": name,
                    "ok": true,
                    "durationMs": started.elapsed().as_millis()
                }),
                Err(error) => json!({
                    "kind": "mcp_tool_call",
                    "tool": name,
                    "ok": false,
                    "code": error.code(),
                    "durationMs": started.elapsed().as_millis()
                }),
            };
            state.audit_mcp(audit).await;
            result
        }
        _ => {
            return Some(json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": -32601, "message": format!("method not found: {method}") }
            }));
        }
    };
    Some(match result {
        Ok(result) => rpc_result(id, result),
        Err(error) => rpc_error(id, &error),
    })
}

fn argument_path(arguments: &Value) -> Result<&str, BridgeError> {
    arguments
        .get("path")
        .and_then(Value::as_str)
        .filter(|path| !path.trim().is_empty())
        .ok_or_else(|| BridgeError::tool("path_required", "path is required"))
}

async fn call_tool(state: &AppState, name: &str, arguments: &Value) -> Result<Value, BridgeError> {
    if name.starts_with("nuphus.") {
        call_nuphus_access_tool(state, name, arguments).await
    } else if name.starts_with("desktop_") || name.starts_with("browser_") {
        call_nuphus_tool(state, name, arguments).await
    } else if name.starts_with("local_exec.") {
        call_execution_tool(state, name, arguments).await
    } else {
        call_file_tool(state, name, arguments).await
    }
}

/// Nuphus 桌面/浏览器自动化工具:先过能力授权门,再执行。
async fn call_nuphus_tool(
    state: &AppState,
    name: &str,
    arguments: &Value,
) -> Result<Value, BridgeError> {
    state.nuphus.require(name, arguments)?;
    match nuphus_mcp_core::tools::execute(name, arguments).await {
        Ok(output) if !output.is_error => Ok(content_text(output.text)),
        Ok(output) => Err(BridgeError::tool(
            "nuphus_tool_error",
            format!("{name}: {}", output.text),
        )),
        Err(error) => Err(BridgeError::tool(
            "nuphus_tool_error",
            format!("{name}: {error}"),
        )),
    }
}

/// Nuphus 能力授权管理工具(nuphus.request_access / access_status / revoke)。
async fn call_nuphus_access_tool(
    state: &AppState,
    name: &str,
    arguments: &Value,
) -> Result<Value, BridgeError> {
    match name {
        "nuphus.request_access" => {
            let capability = argument_string(arguments, "capability", "capability_not_found")?;
            let quote = argument_string(arguments, "quote", "quote_not_found")?;
            let grant = state.nuphus.request(
                capability,
                quote,
                state.runtime.config.filesystem.chat_grant_minutes,
            )?;
            Ok(content_json(json!({ "grant": grant })))
        }
        "nuphus.access_status" => {
            let grants = state.nuphus.list_grants();
            Ok(content_json(json!({ "grants": grants })))
        }
        "nuphus.revoke" => {
            let grant_id = argument_string(arguments, "grantId", "grant_id_not_found")?;
            state.nuphus.revoke(grant_id)?;
            Ok(content_json(json!({ "revoked": grant_id })))
        }
        _ => Err(BridgeError::tool(
            "unknown_tool",
            format!("unknown nuphus tool: {name}"),
        )),
    }
}

fn call_file_tool<'a>(
    state: &'a AppState,
    name: &'a str,
    arguments: &'a Value,
) -> BoxFuture<'a, Result<Value, BridgeError>> {
    Box::pin(async move {
        match name {
            "local_fs.roots" => Ok(content_json(json!({
                "device": state.device,
                "roots": state.access.list_grants().await
            }))),
            "local_fs.request_access" => {
                Ok(content_json(state.access.request_access(arguments).await?))
            }
            "local_fs.access_status" => Ok(content_json(
                state
                    .access
                    .access_status(argument_string(
                        arguments,
                        "requestId",
                        "request_not_found",
                    )?)
                    .await?,
            )),
            "local_fs.list" => list_directory(state, arguments).await,
            "local_fs.stat" => {
                let resolved =
                    state
                        .resolver
                        .resolve(argument_path(arguments)?, AccessMode::Read, false)?;
                Ok(content_json(stat_value(&resolved, false).await?))
            }
            "local_fs.hash" => {
                let resolved =
                    state
                        .resolver
                        .resolve(argument_path(arguments)?, AccessMode::Read, false)?;
                let metadata = tokio::fs::metadata(&resolved.real)
                    .await
                    .map_err(|error| BridgeError::tool("stat_failed", error.to_string()))?;
                if !metadata.is_file() {
                    return Err(BridgeError::tool("target_not_file", "target is not a file"));
                }
                Ok(content_json(json!({
                    "path": public_path(&resolved.grant.id, &resolved.relative),
                    "sha256": sha256_file(&resolved.real).await?
                })))
            }
            "local_fs.read_text" => read_text(state, arguments, false).await,
            "local_fs.read_lines" => read_lines(state, arguments).await,
            "local_fs.read_chunk" => read_chunk(state, arguments).await,
            "local_fs.read_image" => read_image(state, arguments).await,
            "local_fs.search" => search_files(state, arguments).await,
            "local_fs.watch" => start_watch(state, arguments).await,
            "local_fs.watch_events" => watch_events(state, arguments).await,
            "local_fs.unwatch" => stop_watch(state, arguments).await,
            "local_fs.write_text" => write_text(state, arguments).await,
            "local_fs.append_text" => append_text(state, arguments).await,
            "local_fs.apply_patch" => apply_patch(state, arguments).await,
            "local_fs.write_base64" => write_base64(state, arguments).await,
            "local_fs.append_base64" => append_base64(state, arguments).await,
            "local_fs.mkdir" => create_directory(state, arguments).await,
            "local_fs.copy" => copy_or_move(state, arguments, false).await,
            "local_fs.move" => copy_or_move(state, arguments, true).await,
            "local_fs.delete_to_trash" => delete_to_trash(state, arguments).await,
            _ => Err(BridgeError::tool(
                "unknown_tool",
                format!("unknown tool: {name}"),
            )),
        }
    })
}

async fn call_execution_tool(
    state: &AppState,
    name: &str,
    arguments: &Value,
) -> Result<Value, BridgeError> {
    match name {
        "local_exec.runtimes" => Ok(content_json(
            serde_json::to_value(
                state
                    .execution
                    .detect_runtimes(
                        arguments
                            .get("refresh")
                            .and_then(Value::as_bool)
                            .unwrap_or(false),
                    )
                    .await,
            )
            .map_err(|error| BridgeError::tool("runtime_probe_failed", error.to_string()))?,
        )),
        "local_exec.request_run" => Ok(content_json(state.execution.request_run(arguments).await?)),
        "local_exec.execute" => Ok(content_json(state.execution.execute(arguments).await?)),
        "local_exec.request_status" => Ok(content_json(
            state
                .execution
                .request_status(argument_string(
                    arguments,
                    "requestId",
                    "request_not_found",
                )?)
                .await?,
        )),
        "local_exec.authorizations" => Ok(content_json(json!({
            "authorizations": state.execution.list_authorizations().await
        }))),
        "local_exec.run" => Ok(content_json(
            serde_json::to_value(
                state
                    .execution
                    .run_authorization(argument_string(
                        arguments,
                        "authorizationId",
                        "authorization_not_found",
                    )?)
                    .await?,
            )
            .map_err(|error| BridgeError::tool("job_start_failed", error.to_string()))?,
        )),
        "local_exec.job_status" => Ok(content_json(
            serde_json::to_value(
                state
                    .execution
                    .get_job(argument_string(arguments, "jobId", "job_not_found")?)
                    .await?,
            )
            .map_err(|error| BridgeError::tool("job_read_failed", error.to_string()))?,
        )),
        "local_exec.job_output" => Ok(content_json(
            state
                .execution
                .read_job_output(
                    argument_string(arguments, "jobId", "job_not_found")?,
                    arguments,
                )
                .await?,
        )),
        "local_exec.cancel_job" => Ok(content_json(
            serde_json::to_value(
                state
                    .execution
                    .cancel_job(argument_string(arguments, "jobId", "job_not_found")?)
                    .await?,
            )
            .map_err(|error| BridgeError::tool("job_cancel_failed", error.to_string()))?,
        )),
        "local_exec.list_processes" => Ok(content_json(
            state.execution.list_processes(arguments).await?,
        )),
        "local_exec.terminate" => Ok(content_json(state.execution.terminate(arguments).await?)),
        _ => Err(BridgeError::tool(
            "unknown_tool",
            format!("unknown tool: {name}"),
        )),
    }
}

async fn sha256_file(path: &Path) -> Result<String, BridgeError> {
    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(|error| BridgeError::tool("read_failed", error.to_string()))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = file
            .read(&mut buffer)
            .await
            .map_err(|error| BridgeError::tool("read_failed", error.to_string()))?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

async fn stat_value(resolved: &ResolvedPath, include_hash: bool) -> Result<Value, BridgeError> {
    let metadata = tokio::fs::metadata(&resolved.real)
        .await
        .map_err(|error| BridgeError::tool("stat_failed", error.to_string()))?;
    let modified = metadata
        .modified()
        .ok()
        .map(chrono::DateTime::<chrono::Utc>::from)
        .map(|value| value.to_rfc3339());
    let mut value = json!({
        "name": resolved
            .real
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or_else(|| resolved.grant.name.as_str()),
        "path": public_path(&resolved.grant.id, &resolved.relative),
        "type": if metadata.is_dir() { "directory" } else if metadata.is_file() { "file" } else { "other" },
        "size": metadata.len(),
        "modifiedAt": modified,
        "mode": resolved.grant.mode
    });
    if include_hash && metadata.is_file() {
        value["sha256"] = Value::String(sha256_file(&resolved.real).await?);
    }
    Ok(value)
}

async fn list_directory(state: &AppState, arguments: &Value) -> Result<Value, BridgeError> {
    let resolved = state
        .resolver
        .resolve(argument_path(arguments)?, AccessMode::Read, false)?;
    let metadata = tokio::fs::metadata(&resolved.real)
        .await
        .map_err(|error| BridgeError::tool("stat_failed", error.to_string()))?;
    if !metadata.is_dir() {
        return Err(BridgeError::tool(
            "target_not_directory",
            "target is not a directory",
        ));
    }
    let limit = arguments
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(200)
        .clamp(1, 1000) as usize;
    let cursor = arguments
        .get("cursor")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let mut reader = tokio::fs::read_dir(&resolved.real)
        .await
        .map_err(|error| BridgeError::tool("read_failed", error.to_string()))?;
    let mut names = Vec::new();
    while let Some(entry) = reader
        .next_entry()
        .await
        .map_err(|error| BridgeError::tool("read_failed", error.to_string()))?
    {
        names.push(entry.file_name().to_string_lossy().to_string());
    }
    names.sort_by_key(|name| name.to_ascii_lowercase());
    let start = if cursor.is_empty() {
        0
    } else {
        names.partition_point(|name| name.to_ascii_lowercase() <= cursor.to_ascii_lowercase())
    };
    let selected = &names[start..names.len().min(start + limit)];
    let mut entries = Vec::with_capacity(selected.len());
    for name in selected {
        let relative = resolved.relative.join(name);
        let child = state.resolver.resolve(
            &public_path(&resolved.grant.id, &relative),
            AccessMode::Read,
            false,
        )?;
        entries.push(stat_value(&child, false).await?);
    }
    let next_cursor = if start + selected.len() < names.len() {
        selected.last().cloned()
    } else {
        None
    };
    Ok(content_json(json!({
        "path": public_path(&resolved.grant.id, &resolved.relative),
        "mode": resolved.grant.mode,
        "entries": entries,
        "nextCursor": next_cursor,
        "truncated": next_cursor.is_some()
    })))
}

struct DecodedText {
    text: String,
    encoding: &'static str,
    bom: bool,
}

fn decode_text(bytes: &[u8]) -> Result<DecodedText, BridgeError> {
    if bytes.starts_with(&[0xff, 0xfe]) {
        let units: Vec<u16> = bytes[2..]
            .chunks_exact(2)
            .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
            .collect();
        return String::from_utf16(&units)
            .map(|text| DecodedText {
                text,
                encoding: "utf16le",
                bom: true,
            })
            .map_err(|error| BridgeError::tool("invalid_text_encoding", error.to_string()));
    }
    if bytes.starts_with(&[0xfe, 0xff]) {
        let units: Vec<u16> = bytes[2..]
            .chunks_exact(2)
            .map(|chunk| u16::from_be_bytes([chunk[0], chunk[1]]))
            .collect();
        return String::from_utf16(&units)
            .map(|text| DecodedText {
                text,
                encoding: "utf16be",
                bom: true,
            })
            .map_err(|error| BridgeError::tool("invalid_text_encoding", error.to_string()));
    }
    let (content, bom) = if bytes.starts_with(&[0xef, 0xbb, 0xbf]) {
        (&bytes[3..], true)
    } else {
        (bytes, false)
    };
    String::from_utf8(content.to_vec())
        .map(|text| DecodedText {
            text,
            encoding: "utf8",
            bom,
        })
        .map_err(|error| BridgeError::tool("invalid_text_encoding", error.to_string()))
}

async fn read_text(
    state: &AppState,
    arguments: &Value,
    with_metadata: bool,
) -> Result<Value, BridgeError> {
    let resolved = state
        .resolver
        .resolve(argument_path(arguments)?, AccessMode::Read, false)?;
    let bytes = tokio::fs::read(&resolved.real)
        .await
        .map_err(|error| BridgeError::tool("read_failed", error.to_string()))?;
    if bytes.len() > 1024 * 1024 {
        return Err(BridgeError::tool(
            "file_too_large",
            "text file exceeds 1048576 byte limit",
        ));
    }
    let decoded = decode_text(&bytes)?;
    if with_metadata {
        Ok(content_json(json!({
            "path": public_path(&resolved.grant.id, &resolved.relative),
            "text": decoded.text,
            "encoding": decoded.encoding,
            "bom": decoded.bom
        })))
    } else {
        Ok(content_text(decoded.text))
    }
}

async fn read_lines(state: &AppState, arguments: &Value) -> Result<Value, BridgeError> {
    let resolved = state
        .resolver
        .resolve(argument_path(arguments)?, AccessMode::Read, false)?;
    let bytes = tokio::fs::read(&resolved.real)
        .await
        .map_err(|error| BridgeError::tool("read_failed", error.to_string()))?;
    if bytes.len() > 1024 * 1024 {
        return Err(BridgeError::tool(
            "file_too_large",
            "text file exceeds 1048576 byte limit",
        ));
    }
    let decoded = decode_text(&bytes)?;
    let newline = if decoded.text.contains("\r\n") {
        "crlf"
    } else if decoded.text.contains('\r') {
        "cr"
    } else {
        "lf"
    };
    let trimmed = decoded.text.trim_end_matches(['\r', '\n']);
    // `lines()` 保留内部空行(行号必须连续),并正确处理 CRLF;空串返回空迭代器。
    let lines: Vec<&str> = trimmed.lines().collect();
    let start_line = arguments
        .get("startLine")
        .and_then(Value::as_u64)
        .unwrap_or(1)
        .max(1) as usize;
    let line_count = arguments
        .get("lineCount")
        .and_then(Value::as_u64)
        .unwrap_or(200)
        .clamp(1, 2000) as usize;
    let selected: Vec<Value> = lines
        .iter()
        .enumerate()
        .skip(start_line - 1)
        .take(line_count)
        .map(|(index, line)| json!({ "number": index + 1, "text": line }))
        .collect();
    Ok(content_json(json!({
        "path": public_path(&resolved.grant.id, &resolved.relative),
        "encoding": decoded.encoding,
        "bom": decoded.bom,
        "newline": newline,
        "totalLines": lines.len(),
        "startLine": start_line,
        "lines": selected,
        "truncated": start_line - 1 + selected.len() < lines.len()
    })))
}

async fn read_chunk(state: &AppState, arguments: &Value) -> Result<Value, BridgeError> {
    let resolved = state
        .resolver
        .resolve(argument_path(arguments)?, AccessMode::Read, false)?;
    let mut file = tokio::fs::File::open(&resolved.real)
        .await
        .map_err(|error| BridgeError::tool("read_failed", error.to_string()))?;
    let metadata = file
        .metadata()
        .await
        .map_err(|error| BridgeError::tool("stat_failed", error.to_string()))?;
    let offset = arguments.get("offset").and_then(Value::as_u64).unwrap_or(0);
    let length = arguments
        .get("length")
        .and_then(Value::as_u64)
        .unwrap_or(64 * 1024)
        .clamp(1, 1024 * 1024) as usize;
    use tokio::io::AsyncSeekExt;
    file.seek(std::io::SeekFrom::Start(offset))
        .await
        .map_err(|error| BridgeError::tool("read_failed", error.to_string()))?;
    let mut buffer = vec![0u8; length];
    let count = file
        .read(&mut buffer)
        .await
        .map_err(|error| BridgeError::tool("read_failed", error.to_string()))?;
    buffer.truncate(count);
    Ok(content_json(json!({
        "path": public_path(&resolved.grant.id, &resolved.relative),
        "offset": offset,
        "length": count,
        "size": metadata.len(),
        "eof": offset + count as u64 >= metadata.len(),
        "dataBase64": base64::engine::general_purpose::STANDARD.encode(buffer)
    })))
}

fn image_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]) {
        return Some("image/png");
    }
    if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        return Some("image/jpeg");
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some("image/gif");
    }
    if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    None
}

async fn read_image(state: &AppState, arguments: &Value) -> Result<Value, BridgeError> {
    let resolved = state
        .resolver
        .resolve(argument_path(arguments)?, AccessMode::Read, false)?;
    let bytes = tokio::fs::read(&resolved.real)
        .await
        .map_err(|error| BridgeError::tool("read_failed", error.to_string()))?;
    if bytes.len() > 8 * 1024 * 1024 {
        return Err(BridgeError::tool(
            "image_too_large",
            "image exceeds 8388608 byte limit",
        ));
    }
    let mime = image_mime(&bytes).ok_or_else(|| {
        BridgeError::tool(
            "unsupported_image_format",
            "supported formats are PNG, JPEG, GIF, and WebP",
        )
    })?;
    let metadata = json!({
        "name": resolved.real.file_name().and_then(|value| value.to_str()).unwrap_or("image"),
        "path": public_path(&resolved.grant.id, &resolved.relative),
        "size": bytes.len(),
        "mimeType": mime,
        "sha256": format!("{:x}", Sha256::digest(&bytes))
    });
    Ok(json!({
        "content": [
            { "type": "text", "text": serde_json::to_string_pretty(&metadata).unwrap() },
            {
                "type": "image",
                "mimeType": mime,
                "data": base64::engine::general_purpose::STANDARD.encode(bytes)
            }
        ]
    }))
}

fn argument_string<'a>(
    arguments: &'a Value,
    name: &str,
    code: &'static str,
) -> Result<&'a str, BridgeError> {
    arguments
        .get(name)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| BridgeError::tool(code, format!("{name} is required")))
}

fn argument_usize(arguments: &Value, name: &str, fallback: usize, min: usize, max: usize) -> usize {
    arguments
        .get(name)
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .unwrap_or(fallback)
        .clamp(min, max)
}

fn normalize_encoding(
    value: Option<&str>,
    fallback: &'static str,
) -> Result<&'static str, BridgeError> {
    match value
        .unwrap_or(fallback)
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "utf8" | "utf-8" => Ok("utf8"),
        "utf16le" | "utf-16le" => Ok("utf16le"),
        "utf16be" | "utf-16be" => Ok("utf16be"),
        _ => Err(BridgeError::tool(
            "unsupported_text_encoding",
            "encoding must be utf8, utf16le, or utf16be",
        )),
    }
}

fn encode_text(text: &str, encoding: &str, bom: bool) -> Vec<u8> {
    match encoding {
        "utf16le" => {
            let mut bytes = Vec::with_capacity(text.len() * 2 + usize::from(bom) * 2);
            if bom {
                bytes.extend_from_slice(&[0xff, 0xfe]);
            }
            for unit in text.encode_utf16() {
                bytes.extend_from_slice(&unit.to_le_bytes());
            }
            bytes
        }
        "utf16be" => {
            let mut bytes = Vec::with_capacity(text.len() * 2 + usize::from(bom) * 2);
            if bom {
                bytes.extend_from_slice(&[0xfe, 0xff]);
            }
            for unit in text.encode_utf16() {
                bytes.extend_from_slice(&unit.to_be_bytes());
            }
            bytes
        }
        _ => {
            let mut bytes = Vec::with_capacity(text.len() + usize::from(bom) * 3);
            if bom {
                bytes.extend_from_slice(&[0xef, 0xbb, 0xbf]);
            }
            bytes.extend_from_slice(text.as_bytes());
            bytes
        }
    }
}

fn sha256_file_blocking(path: &Path) -> Result<String, BridgeError> {
    let mut file = fs::File::open(path)
        .map_err(|error| BridgeError::tool("read_failed", error.to_string()))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = std::io::Read::read(&mut file, &mut buffer)
            .map_err(|error| BridgeError::tool("read_failed", error.to_string()))?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

async fn atomic_write(
    path: PathBuf,
    bytes: Vec<u8>,
    overwrite: bool,
    expected_sha256: Option<String>,
    create_parents: bool,
) -> Result<bool, BridgeError> {
    tokio::task::spawn_blocking(move || -> Result<bool, BridgeError> {
        let parent = path.parent().ok_or_else(|| {
            BridgeError::tool("parent_not_found", "destination parent does not exist")
        })?;
        if create_parents {
            fs::create_dir_all(parent)
                .map_err(|error| BridgeError::tool("mkdir_failed", error.to_string()))?;
        }
        if !parent.is_dir() {
            return Err(BridgeError::tool(
                "parent_not_found",
                "destination parent does not exist",
            ));
        }
        let existed = path.exists();
        if existed && !overwrite {
            return Err(BridgeError::tool(
                "overwrite_required",
                "target already exists; set overwrite and provide expectedSha256",
            ));
        }
        if existed {
            let expected = expected_sha256.as_deref().ok_or_else(|| {
                BridgeError::tool(
                    "expected_sha256_required",
                    "expectedSha256 is required when overwriting an existing file",
                )
            })?;
            let actual = sha256_file_blocking(&path)?;
            if !actual.eq_ignore_ascii_case(expected) {
                return Err(BridgeError::mismatch(
                    "sha256_mismatch",
                    "target changed since it was inspected",
                    expected,
                    actual,
                ));
            }
        }

        let identifier = Uuid::new_v4().simple().to_string();
        let stem = path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("hanako-file");
        let temporary = parent.join(format!(".{stem}.{identifier}.tmp"));
        let backup = parent.join(format!(".{stem}.{identifier}.backup"));
        let write_result = (|| -> Result<(), BridgeError> {
            let mut file = fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&temporary)
                .map_err(|error| BridgeError::tool("write_failed", error.to_string()))?;
            file.write_all(&bytes)
                .and_then(|()| file.sync_all())
                .map_err(|error| BridgeError::tool("write_failed", error.to_string()))?;
            drop(file);

            if existed {
                fs::rename(&path, &backup)
                    .map_err(|error| BridgeError::tool("replace_failed", error.to_string()))?;
            }
            if let Err(error) = fs::rename(&temporary, &path) {
                if existed {
                    let _ = fs::rename(&backup, &path);
                }
                return Err(BridgeError::tool("replace_failed", error.to_string()));
            }
            if existed {
                let _ = fs::remove_file(&backup);
            }
            Ok(())
        })();
        let _ = fs::remove_file(&temporary);
        if write_result.is_err() && existed && backup.exists() && !path.exists() {
            let _ = fs::rename(&backup, &path);
        }
        write_result?;
        Ok(existed)
    })
    .await
    .map_err(|error| BridgeError::tool("write_failed", error.to_string()))?
}

async fn write_text(state: &AppState, arguments: &Value) -> Result<Value, BridgeError> {
    let resolved =
        state
            .resolver
            .resolve(argument_path(arguments)?, AccessMode::ReadWrite, true)?;
    let _guard = state.lock_path(&resolved.real).await;
    let current = if resolved.real.exists() {
        Some(
            tokio::fs::read(&resolved.real)
                .await
                .map_err(|error| BridgeError::tool("read_failed", error.to_string()))?,
        )
    } else {
        None
    };
    let decoded = current.as_deref().map(decode_text).transpose()?;
    let encoding = normalize_encoding(
        arguments.get("encoding").and_then(Value::as_str),
        decoded.as_ref().map_or("utf8", |value| value.encoding),
    )?;
    let bom = arguments
        .get("bom")
        .and_then(Value::as_bool)
        .unwrap_or_else(|| {
            decoded
                .as_ref()
                .map_or(encoding != "utf8", |value| value.bom)
        });
    let bytes = encode_text(
        arguments.get("text").and_then(Value::as_str).unwrap_or(""),
        encoding,
        bom,
    );
    if bytes.len() > 4 * 1024 * 1024 {
        return Err(BridgeError::tool(
            "write_too_large",
            "write exceeds 4194304 byte limit",
        ));
    }
    atomic_write(
        resolved.real.clone(),
        bytes,
        arguments
            .get("overwrite")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        arguments
            .get("expectedSha256")
            .and_then(Value::as_str)
            .map(str::to_string),
        arguments
            .get("createParents")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    )
    .await?;
    let final_resolved =
        state
            .resolver
            .resolve(argument_path(arguments)?, AccessMode::ReadWrite, false)?;
    let mut stat = stat_value(&final_resolved, true).await?;
    stat["encoding"] = Value::String(encoding.to_string());
    stat["bom"] = Value::Bool(bom);
    Ok(content_json(stat))
}

async fn append_text(state: &AppState, arguments: &Value) -> Result<Value, BridgeError> {
    let resolved =
        state
            .resolver
            .resolve(argument_path(arguments)?, AccessMode::ReadWrite, true)?;
    let _guard = state.lock_path(&resolved.real).await;
    let existed = resolved.real.exists();
    let (mut text, encoding, bom, current_sha) = if existed {
        let bytes = tokio::fs::read(&resolved.real)
            .await
            .map_err(|error| BridgeError::tool("read_failed", error.to_string()))?;
        if bytes.len() > 4 * 1024 * 1024 {
            return Err(BridgeError::tool(
                "file_too_large",
                "existing file is too large for append_text",
            ));
        }
        let decoded = decode_text(&bytes)?;
        if let Some(requested) = arguments.get("encoding").and_then(Value::as_str)
            && normalize_encoding(Some(requested), decoded.encoding)? != decoded.encoding
        {
            return Err(BridgeError::mismatch(
                "encoding_mismatch",
                "append encoding does not match the existing file",
                decoded.encoding,
                requested,
            ));
        }
        (
            decoded.text,
            decoded.encoding,
            decoded.bom,
            Some(format!("{:x}", Sha256::digest(&bytes))),
        )
    } else {
        let encoding =
            normalize_encoding(arguments.get("encoding").and_then(Value::as_str), "utf8")?;
        (
            String::new(),
            encoding,
            arguments
                .get("bom")
                .and_then(Value::as_bool)
                .unwrap_or(encoding != "utf8"),
            None,
        )
    };
    if let (Some(expected), Some(actual)) = (
        arguments.get("expectedSha256").and_then(Value::as_str),
        current_sha.as_deref(),
    ) && !actual.eq_ignore_ascii_case(expected)
    {
        return Err(BridgeError::mismatch(
            "sha256_mismatch",
            "target changed since it was inspected",
            expected,
            actual,
        ));
    }
    text.push_str(arguments.get("text").and_then(Value::as_str).unwrap_or(""));
    let bytes = encode_text(&text, encoding, bom);
    if bytes.len() > 4 * 1024 * 1024 {
        return Err(BridgeError::tool(
            "write_too_large",
            "write exceeds 4194304 byte limit",
        ));
    }
    atomic_write(
        resolved.real.clone(),
        bytes,
        existed,
        current_sha,
        arguments
            .get("createParents")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    )
    .await?;
    let final_resolved =
        state
            .resolver
            .resolve(argument_path(arguments)?, AccessMode::ReadWrite, false)?;
    let mut stat = stat_value(&final_resolved, true).await?;
    stat["encoding"] = Value::String(encoding.to_string());
    stat["bom"] = Value::Bool(bom);
    Ok(content_json(stat))
}

async fn apply_patch(state: &AppState, arguments: &Value) -> Result<Value, BridgeError> {
    let expected = argument_string(arguments, "expectedSha256", "expected_sha256_required")?;
    let edits = arguments
        .get("edits")
        .and_then(Value::as_array)
        .ok_or_else(|| BridgeError::tool("invalid_edits", "edits must be an array"))?;
    if edits.is_empty() {
        return Err(BridgeError::tool(
            "invalid_edits",
            "edits must not be empty",
        ));
    }
    let resolved =
        state
            .resolver
            .resolve(argument_path(arguments)?, AccessMode::ReadWrite, false)?;
    let _guard = state.lock_path(&resolved.real).await;
    let bytes = tokio::fs::read(&resolved.real)
        .await
        .map_err(|error| BridgeError::tool("read_failed", error.to_string()))?;
    if bytes.len() > 4 * 1024 * 1024 {
        return Err(BridgeError::tool(
            "file_too_large",
            "file is too large for apply_patch",
        ));
    }
    let actual = format!("{:x}", Sha256::digest(&bytes));
    if !actual.eq_ignore_ascii_case(expected) {
        return Err(BridgeError::mismatch(
            "sha256_mismatch",
            "target changed since it was inspected",
            expected,
            actual,
        ));
    }
    let decoded = decode_text(&bytes)?;
    let mut text = decoded.text;
    let mut replacements = 0usize;
    for edit in edits {
        let old_text = edit
            .get("oldText")
            .and_then(Value::as_str)
            .ok_or_else(|| BridgeError::tool("invalid_edits", "oldText is required"))?;
        let new_text = edit
            .get("newText")
            .and_then(Value::as_str)
            .ok_or_else(|| BridgeError::tool("invalid_edits", "newText is required"))?;
        if old_text.is_empty() {
            return Err(BridgeError::tool(
                "invalid_edits",
                "oldText must not be empty",
            ));
        }
        let occurrences = text.match_indices(old_text).count();
        let expected_occurrences = edit
            .get("expectedOccurrences")
            .and_then(Value::as_u64)
            .and_then(|value| usize::try_from(value).ok())
            .unwrap_or(1);
        if occurrences != expected_occurrences {
            return Err(BridgeError::mismatch(
                "occurrence_count_mismatch",
                "oldText occurrence count does not match expectedOccurrences",
                expected_occurrences.to_string(),
                occurrences.to_string(),
            ));
        }
        text = text.replace(old_text, new_text);
        replacements += occurrences;
    }
    atomic_write(
        resolved.real.clone(),
        encode_text(&text, decoded.encoding, decoded.bom),
        true,
        Some(actual),
        false,
    )
    .await?;
    let final_resolved =
        state
            .resolver
            .resolve(argument_path(arguments)?, AccessMode::ReadWrite, false)?;
    let mut stat = stat_value(&final_resolved, true).await?;
    stat["encoding"] = Value::String(decoded.encoding.to_string());
    stat["bom"] = Value::Bool(decoded.bom);
    stat["replacements"] = json!(replacements);
    Ok(content_json(stat))
}

async fn write_base64(state: &AppState, arguments: &Value) -> Result<Value, BridgeError> {
    let raw = arguments
        .get("dataBase64")
        .and_then(Value::as_str)
        .ok_or_else(|| BridgeError::tool("invalid_base64", "dataBase64 is required"))?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(raw)
        .map_err(|error| BridgeError::tool("invalid_base64", error.to_string()))?;
    if bytes.len() > 4 * 1024 * 1024 {
        return Err(BridgeError::tool(
            "write_too_large",
            "write exceeds 4194304 byte limit",
        ));
    }
    let resolved =
        state
            .resolver
            .resolve(argument_path(arguments)?, AccessMode::ReadWrite, true)?;
    let _guard = state.lock_path(&resolved.real).await;
    atomic_write(
        resolved.real,
        bytes,
        arguments
            .get("overwrite")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        arguments
            .get("expectedSha256")
            .and_then(Value::as_str)
            .map(str::to_string),
        arguments
            .get("createParents")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    )
    .await?;
    let final_resolved =
        state
            .resolver
            .resolve(argument_path(arguments)?, AccessMode::ReadWrite, false)?;
    Ok(content_json(stat_value(&final_resolved, true).await?))
}

/// Appends base64-decoded bytes to a local file, creating it if missing.
///
/// Designed for transferring large binary files in chunks: an agent that
/// cannot reliably emit a whole base64 payload in one call splits it into
/// several sequential `append_base64` calls against the same path. Each call
/// appends only its own chunk, so the total wire traffic stays close to the
/// file size (unlike rewriting the whole file per chunk). Use `finalSha256`
/// on the last chunk to verify the complete assembled file.
async fn append_base64(state: &AppState, arguments: &Value) -> Result<Value, BridgeError> {
    const MAX_APPEND_BYTES: usize = 64 * 1024 * 1024;
    let raw = arguments
        .get("dataBase64")
        .and_then(Value::as_str)
        .ok_or_else(|| BridgeError::tool("invalid_base64", "dataBase64 is required"))?;
    let chunk = base64::engine::general_purpose::STANDARD
        .decode(raw)
        .map_err(|error| BridgeError::tool("invalid_base64", error.to_string()))?;
    if chunk.len() > 4 * 1024 * 1024 {
        return Err(BridgeError::tool(
            "write_too_large",
            "append chunk exceeds 4194304 byte limit",
        ));
    }
    let resolved =
        state
            .resolver
            .resolve(argument_path(arguments)?, AccessMode::ReadWrite, true)?;
    let _guard = state.lock_path(&resolved.real).await;

    let create_parents = arguments
        .get("createParents")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if create_parents
        && let Some(parent) = resolved.real.parent()
        && !parent.as_os_str().is_empty()
        && !parent.exists()
    {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|error| BridgeError::tool("mkdir_failed", error.to_string()))?;
    }

    // Optional precondition: the file as it exists before this append.
    if let Some(expected) = arguments.get("expectedSha256").and_then(Value::as_str) {
        let actual = if resolved.real.exists() {
            let bytes = tokio::fs::read(&resolved.real)
                .await
                .map_err(|error| BridgeError::tool("read_failed", error.to_string()))?;
            format!("{:x}", Sha256::digest(&bytes))
        } else {
            String::new()
        };
        if !actual.eq_ignore_ascii_case(expected) {
            return Err(BridgeError::mismatch(
                "sha256_mismatch",
                "target changed since it was inspected",
                expected,
                actual,
            ));
        }
    }

    // Size guard: keep the whole assembled file bounded.
    let existing_len = tokio::fs::metadata(&resolved.real)
        .await
        .map(|meta| meta.len() as usize)
        .unwrap_or(0);
    let final_len = existing_len
        .checked_add(chunk.len())
        .ok_or_else(|| BridgeError::tool("write_too_large", "file size overflow"))?;
    if final_len > MAX_APPEND_BYTES {
        return Err(BridgeError::tool(
            "write_too_large",
            "assembled file would exceed 67108864 byte limit",
        ));
    }

    let mut options = tokio::fs::OpenOptions::new();
    options.create(true).append(true);
    {
        let mut file = options
            .open(&resolved.real)
            .await
            .map_err(|error| BridgeError::tool("open_failed", error.to_string()))?;
        use tokio::io::AsyncWriteExt as _;
        file.write_all(&chunk)
            .await
            .map_err(|error| BridgeError::tool("append_failed", error.to_string()))?;
        file.flush()
            .await
            .map_err(|error| BridgeError::tool("append_failed", error.to_string()))?;
    }

    // Optional postcondition: verify the complete assembled file.
    if let Some(expected) = arguments.get("finalSha256").and_then(Value::as_str) {
        let bytes = tokio::fs::read(&resolved.real)
            .await
            .map_err(|error| BridgeError::tool("read_failed", error.to_string()))?;
        let actual = format!("{:x}", Sha256::digest(&bytes));
        if !actual.eq_ignore_ascii_case(expected) {
            return Err(BridgeError::mismatch(
                "sha256_mismatch",
                "final sha256 does not match assembled file",
                expected,
                actual,
            ));
        }
    }

    let final_resolved =
        state
            .resolver
            .resolve(argument_path(arguments)?, AccessMode::ReadWrite, false)?;
    Ok(content_json(stat_value(&final_resolved, true).await?))
}

async fn create_directory(state: &AppState, arguments: &Value) -> Result<Value, BridgeError> {
    let resolved =
        state
            .resolver
            .resolve(argument_path(arguments)?, AccessMode::ReadWrite, true)?;
    let _guard = state.lock_path(&resolved.real).await;
    if arguments
        .get("recursive")
        .and_then(Value::as_bool)
        .unwrap_or(true)
    {
        tokio::fs::create_dir_all(&resolved.real)
            .await
            .map_err(|error| BridgeError::tool("mkdir_failed", error.to_string()))?;
    } else {
        tokio::fs::create_dir(&resolved.real)
            .await
            .map_err(|error| BridgeError::tool("mkdir_failed", error.to_string()))?;
    }
    let final_resolved =
        state
            .resolver
            .resolve(argument_path(arguments)?, AccessMode::ReadWrite, false)?;
    Ok(content_json(stat_value(&final_resolved, false).await?))
}

fn copy_recursively(source: &Path, destination: &Path) -> Result<(), BridgeError> {
    let metadata = fs::symlink_metadata(source)
        .map_err(|error| BridgeError::tool("copy_failed", error.to_string()))?;
    if metadata.file_type().is_symlink() {
        return Err(BridgeError::tool(
            "symlink_not_supported",
            "symbolic links are not copied",
        ));
    }
    if metadata.is_file() {
        fs::copy(source, destination)
            .map_err(|error| BridgeError::tool("copy_failed", error.to_string()))?;
        return Ok(());
    }
    fs::create_dir(destination)
        .map_err(|error| BridgeError::tool("copy_failed", error.to_string()))?;
    for entry in
        fs::read_dir(source).map_err(|error| BridgeError::tool("copy_failed", error.to_string()))?
    {
        let entry = entry.map_err(|error| BridgeError::tool("copy_failed", error.to_string()))?;
        copy_recursively(&entry.path(), &destination.join(entry.file_name()))?;
    }
    Ok(())
}

async fn copy_or_move(
    state: &AppState,
    arguments: &Value,
    move_source: bool,
) -> Result<Value, BridgeError> {
    let source_input = argument_string(arguments, "source", "source_required")?;
    let destination_input = argument_string(arguments, "destination", "destination_required")?;
    let source_mode = if move_source {
        AccessMode::ReadWrite
    } else {
        AccessMode::Read
    };
    let source = state.resolver.resolve(source_input, source_mode, false)?;
    let destination = state
        .resolver
        .resolve(destination_input, AccessMode::ReadWrite, true)?;
    let source_key = source
        .real
        .to_string_lossy()
        .replace('/', "\\")
        .to_ascii_lowercase();
    let destination_key = destination
        .real
        .to_string_lossy()
        .replace('/', "\\")
        .to_ascii_lowercase();
    if source_key == destination_key {
        return Err(BridgeError::tool(
            "destination_exists",
            "destination already exists",
        ));
    }
    // 固定锁获取顺序,避免并发 copy/move(X→Y 与 Y→X)互相持锁等待形成死锁。
    let (_source_guard, _destination_guard) = if source_key <= destination_key {
        let source_guard = state.lock_path(&source.real).await;
        let destination_guard = state.lock_path(&destination.real).await;
        (source_guard, destination_guard)
    } else {
        let destination_guard = state.lock_path(&destination.real).await;
        let source_guard = state.lock_path(&source.real).await;
        (source_guard, destination_guard)
    };
    if destination.real.exists() {
        return Err(BridgeError::tool(
            "destination_exists",
            "destination already exists",
        ));
    }
    let create_parents = arguments
        .get("createParents")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let source_path = source.real.clone();
    let destination_path = destination.real.clone();
    tokio::task::spawn_blocking(move || -> Result<(), BridgeError> {
        let parent = destination_path.parent().ok_or_else(|| {
            BridgeError::tool("parent_not_found", "destination parent does not exist")
        })?;
        if create_parents {
            fs::create_dir_all(parent)
                .map_err(|error| BridgeError::tool("mkdir_failed", error.to_string()))?;
        }
        if !parent.is_dir() {
            return Err(BridgeError::tool(
                "parent_not_found",
                "destination parent does not exist",
            ));
        }
        if move_source && fs::rename(&source_path, &destination_path).is_ok() {
            return Ok(());
        }
        copy_recursively(&source_path, &destination_path)?;
        if move_source {
            let metadata = fs::symlink_metadata(&source_path)
                .map_err(|error| BridgeError::tool("move_failed", error.to_string()))?;
            if metadata.is_dir() {
                fs::remove_dir_all(&source_path)
                    .map_err(|error| BridgeError::tool("move_failed", error.to_string()))?;
            } else {
                fs::remove_file(&source_path)
                    .map_err(|error| BridgeError::tool("move_failed", error.to_string()))?;
            }
        }
        Ok(())
    })
    .await
    .map_err(|error| BridgeError::tool("copy_failed", error.to_string()))??;
    let final_resolved = state
        .resolver
        .resolve(destination_input, AccessMode::ReadWrite, false)?;
    Ok(content_json(stat_value(&final_resolved, false).await?))
}

async fn delete_to_trash(state: &AppState, arguments: &Value) -> Result<Value, BridgeError> {
    let resolved =
        state
            .resolver
            .resolve(argument_path(arguments)?, AccessMode::ReadWrite, false)?;
    if resolved.relative.as_os_str().is_empty() {
        return Err(BridgeError::tool(
            "root_delete_blocked",
            "an authorized root cannot be deleted",
        ));
    }
    let _guard = state.lock_path(&resolved.real).await;
    let trash_root = if resolved.grant.source == "full_trust" {
        resolved
            .real
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(".hana-trash")
    } else {
        resolved.grant.path.join(".hana-trash")
    };
    tokio::fs::create_dir_all(&trash_root)
        .await
        .map_err(|error| BridgeError::tool("trash_failed", error.to_string()))?;
    let stamp = chrono::Utc::now().format("%Y-%m-%dT%H-%M-%S-%3fZ");
    let name = resolved
        .real
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("item");
    let trash_name = format!(
        "{stamp}-{}-{name}",
        &Uuid::new_v4().simple().to_string()[..6]
    );
    let destination = trash_root.join(&trash_name);
    let source = resolved.real.clone();
    tokio::task::spawn_blocking(move || -> Result<(), BridgeError> {
        if fs::rename(&source, &destination).is_ok() {
            return Ok(());
        }
        copy_recursively(&source, &destination)?;
        if source.is_dir() {
            fs::remove_dir_all(&source)
                .map_err(|error| BridgeError::tool("trash_failed", error.to_string()))?;
        } else {
            fs::remove_file(&source)
                .map_err(|error| BridgeError::tool("trash_failed", error.to_string()))?;
        }
        Ok(())
    })
    .await
    .map_err(|error| BridgeError::tool("trash_failed", error.to_string()))??;
    Ok(content_json(json!({
        "deleted": public_path(&resolved.grant.id, &resolved.relative),
        "recoverable": true,
        "trashName": trash_name
    })))
}

fn glob_matcher(pattern: Option<&str>) -> Result<Option<GlobMatcher>, BridgeError> {
    pattern
        .filter(|value| !value.trim().is_empty())
        .map(|value| {
            Glob::new(value)
                .map(|glob| glob.compile_matcher())
                .map_err(|error| BridgeError::tool("invalid_glob", error.to_string()))
        })
        .transpose()
}

/// Best-effort text decoding for content search.
///
/// Handles BOMs (UTF-8, UTF-16LE, UTF-16BE), BOM-less UTF-16LE (common on
/// Windows), and falls back to UTF-8 with lossy conversion. Returns `None`
/// for binary-looking content (NUL byte density) so search skips it.
fn decode_search_text(bytes: &[u8]) -> Option<String> {
    if bytes.is_empty() {
        return Some(String::new());
    }
    // BOMs are strong signals: handle them before the binary sniff, since
    // UTF-16 text naturally has ~50% NUL bytes and would look binary.
    if bytes.starts_with(&[0xff, 0xfe]) {
        let units: Vec<u16> = bytes[2..]
            .chunks_exact(2)
            .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
            .collect();
        return String::from_utf16(&units).ok();
    }
    if bytes.starts_with(&[0xfe, 0xff]) {
        let units: Vec<u16> = bytes[2..]
            .chunks_exact(2)
            .map(|chunk| u16::from_be_bytes([chunk[0], chunk[1]]))
            .collect();
        return String::from_utf16(&units).ok();
    }
    let body = if bytes.starts_with(&[0xef, 0xbb, 0xbf]) {
        &bytes[3..]
    } else {
        bytes
    };
    // BOM-less UTF-16LE heuristic first: even length and a strong pattern of
    // NUL-at-odd / ASCII-at-even positions identifies UTF-16 before the
    // generic NUL-density gate (which would otherwise reject it as binary).
    if body.len() >= 2 && body.len() % 2 == 0 {
        let units: Vec<u16> = body
            .chunks_exact(2)
            .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
            .collect();
        let probe_units = &units[..units.len().min(4096)];
        let ascii_ish = probe_units
            .iter()
            .filter(|&&u| (0x20..=0x7e).contains(&u) || u == 0)
            .count();
        let pairs_ok = probe_units
            .iter()
            .filter(|&&u| (0x20..=0x7e).contains(&u))
            .count()
            * 10
            >= probe_units.len() * 8;
        if pairs_ok && ascii_ish * 10 >= probe_units.len() * 9 {
            if let Ok(text) = String::from_utf16(&units) {
                return Some(text);
            }
        }
    }
    // Binary sniff on the remaining content: NUL bytes in the first 8KB
    // strongly imply non-text.
    let probe = &body[..body.len().min(8192)];
    let nul_count = probe.iter().filter(|&&b| b == 0).count();
    if nul_count * 10 > probe.len() {
        return None;
    }
    if let Ok(text) = std::str::from_utf8(body) {
        return Some(text.to_string());
    }
    Some(String::from_utf8_lossy(body).into_owned())
}

/// Collects matching lines for content search.
///
/// `needle` is optional (case-insensitive substring); `regex` is optional
/// (matched per line). Returns up to `max_matches` hit lines, each with its
/// 1-based line number and a trimmed snippet (long lines truncated).
fn content_hits(
    text: &str,
    needle: Option<&str>,
    regex: Option<&regex::Regex>,
    max_matches: usize,
) -> Vec<Value> {
    const SNIPPET_MAX: usize = 240;
    const MAX_LINES: usize = 50_000;
    let mut hits = Vec::new();
    let needle_lower = needle.map(str::to_ascii_lowercase);
    for (index, raw_line) in text.split('\n').enumerate() {
        if index >= MAX_LINES {
            break;
        }
        let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        let matches_needle = needle_lower
            .as_ref()
            .is_none_or(|needle| line.to_ascii_lowercase().contains(needle));
        let matches_regex = regex.as_ref().is_none_or(|re| re.is_match(line));
        if !matches_needle || !matches_regex {
            continue;
        }
        let snippet = if line.chars().count() > SNIPPET_MAX {
            let trimmed: String = line.chars().take(SNIPPET_MAX).collect();
            format!("{trimmed}…")
        } else {
            line.to_string()
        };
        hits.push(json!({
            "line": index + 1,
            "snippet": snippet
        }));
        if hits.len() >= max_matches {
            break;
        }
    }
    hits
}

async fn search_files(state: &AppState, arguments: &Value) -> Result<Value, BridgeError> {
    let resolved = state
        .resolver
        .resolve(argument_path(arguments)?, AccessMode::Read, false)?;
    if !resolved.real.is_dir() {
        return Err(BridgeError::tool(
            "target_not_directory",
            "search path must be a directory",
        ));
    }
    let query = arguments
        .get("query")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_ascii_lowercase();
    let matcher = glob_matcher(arguments.get("glob").and_then(Value::as_str))?;
    let exclude = arguments
        .get("exclude")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(|value| glob_matcher(Some(value)))
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    let max_depth = argument_usize(arguments, "maxDepth", 6, 0, 64);
    let limit = argument_usize(arguments, "limit", 100, 1, 1000);
    let timeout_ms = argument_usize(arguments, "timeoutMs", 5000, 100, 30_000);
    let max_visited = argument_usize(arguments, "maxVisited", 10_000, 1, 100_000);
    let content_needle = arguments
        .get("content")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty());
    let content_regex = match arguments
        .get("contentRegex")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    {
        Some(pattern) => Some(
            regex::Regex::new(pattern)
                .map_err(|error| BridgeError::tool("invalid_content_regex", error.to_string()))?,
        ),
        None => None,
    };
    let content_max_bytes = argument_usize(
        arguments,
        "contentMaxBytes",
        1_048_576,
        1024,
        64 * 1024 * 1024,
    );
    let search_content = content_needle.is_some() || content_regex.is_some();
    let root = resolved.real.clone();
    let grant_id = resolved.grant.id.clone();
    let grant_relative = resolved.relative.clone();
    let content_needle_owned = content_needle.map(str::to_string);
    let content_regex_owned = content_regex.clone();
    let search_content_owned = search_content;
    let content_max_bytes_owned = content_max_bytes;
    let result = tokio::task::spawn_blocking(move || {
        let started = std::time::Instant::now();
        let mut results = Vec::new();
        let mut visited = 0usize;
        let mut skipped_links = 0usize;
        let mut visited_directories = HashSet::new();
        let mut content_scanned = 0usize;
        let mut content_skipped_large = 0usize;
        let mut content_skipped_binary = 0usize;
        let mut timed_out = false;
        let mut budget_exceeded = false;

        for entry in WalkDir::new(&root)
            .follow_links(false)
            .max_depth(max_depth + 1)
            .into_iter()
            .filter_entry(|entry| entry.file_name() != ".hana-trash")
        {
            if started.elapsed() >= Duration::from_millis(timeout_ms as u64) {
                timed_out = true;
                break;
            }
            let Ok(entry) = entry else {
                continue;
            };
            if entry.depth() == 0 {
                continue;
            }
            visited += 1;
            if visited > max_visited {
                budget_exceeded = true;
                break;
            }
            if entry.file_type().is_symlink() {
                skipped_links += 1;
                continue;
            }
            if entry.file_type().is_dir() {
                visited_directories.insert(entry.path().to_path_buf());
            }
            let relative = entry.path().strip_prefix(&root).unwrap_or(entry.path());
            let relative_text = relative.to_string_lossy().replace('\\', "/");
            if exclude
                .iter()
                .any(|pattern| pattern.is_match(&relative_text))
            {
                continue;
            }
            let query_matches = query.is_empty()
                || entry
                    .file_name()
                    .to_string_lossy()
                    .to_ascii_lowercase()
                    .contains(&query);
            let glob_matches = matcher
                .as_ref()
                .is_none_or(|pattern| pattern.is_match(&relative_text));
            if !query_matches || !glob_matches {
                continue;
            }
            let metadata = match entry.metadata() {
                Ok(metadata) => metadata,
                Err(_) => continue,
            };
            let mut content_matches = Vec::new();
            if search_content_owned && metadata.is_file() && metadata.len() <= content_max_bytes_owned as u64 {
                match std::fs::read(entry.path()) {
                    Ok(bytes) => {
                        content_scanned += 1;
                        match decode_search_text(&bytes) {
                            Some(text) => {
                                content_matches = content_hits(
                                    &text,
                                    content_needle_owned.as_deref(),
                                    content_regex_owned.as_ref(),
                                    5,
                                );
                            }
                            None => content_skipped_binary += 1,
                        }
                    }
                    Err(_) => {}
                }
            } else if search_content_owned && metadata.is_file() && metadata.len() > content_max_bytes_owned as u64 {
                content_skipped_large += 1;
            }
            if search_content_owned && content_matches.is_empty() {
                continue;
            }
            let mut item = json!({
                "name": entry.file_name().to_string_lossy(),
                "path": public_path(&grant_id, &grant_relative.join(relative)),
                "type": if metadata.is_dir() { "directory" } else if metadata.is_file() { "file" } else { "other" },
                "size": metadata.len(),
                "modifiedAt": metadata.modified().ok().map(chrono::DateTime::<chrono::Utc>::from).map(|value| value.to_rfc3339())
            });
            if search_content_owned {
                item["contentMatches"] = Value::Array(content_matches);
            }
            results.push(item);
            if results.len() >= limit {
                break;
            }
        }
        let mut reasons = Vec::new();
        if results.len() >= limit {
            reasons.push("result_limit");
        }
        if timed_out {
            reasons.push("timeout");
        }
        if budget_exceeded {
            reasons.push("visit_budget");
        }
        let mut output = json!({
            "query": if query.is_empty() { Value::Null } else { Value::String(query) },
            "results": results,
            "visited": visited,
            "visitedDirectories": visited_directories.len(),
            "skippedLinks": skipped_links,
            "elapsedMs": started.elapsed().as_millis(),
            "truncated": !reasons.is_empty(),
            "truncationReasons": reasons
        });
        if search_content_owned {
            output["content"] = Value::String(content_needle_owned.clone().unwrap_or_default());
            output["contentScanned"] = json!(content_scanned);
            output["contentSkippedLarge"] = json!(content_skipped_large);
            output["contentSkippedBinary"] = json!(content_skipped_binary);
        }
        output
    })
    .await
    .map_err(|error| BridgeError::tool("search_failed", error.to_string()))?;
    Ok(content_json(result))
}

fn fingerprint(metadata: &fs::Metadata) -> FileFingerprint {
    let modified_ms = metadata
        .modified()
        .ok()
        .and_then(|value| value.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map_or(0, |value| value.as_millis());
    FileFingerprint {
        is_dir: metadata.is_dir(),
        len: metadata.len(),
        modified_ms,
    }
}

async fn snapshot_path(
    path: PathBuf,
    recursive: bool,
) -> Result<HashMap<String, FileFingerprint>, BridgeError> {
    tokio::task::spawn_blocking(move || {
        let metadata = fs::metadata(&path)
            .map_err(|error| BridgeError::tool("watch_failed", error.to_string()))?;
        let mut snapshot = HashMap::new();
        if metadata.is_file() {
            snapshot.insert(String::new(), fingerprint(&metadata));
            return Ok(snapshot);
        }
        let max_depth = if recursive { 65 } else { 1 };
        for entry in WalkDir::new(&path)
            .follow_links(false)
            .max_depth(max_depth)
            .into_iter()
            .filter_entry(|entry| entry.file_name() != ".hana-trash")
        {
            let Ok(entry) = entry else {
                continue;
            };
            if entry.depth() == 0 || entry.file_type().is_symlink() {
                continue;
            }
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            let relative = entry
                .path()
                .strip_prefix(&path)
                .unwrap_or(entry.path())
                .to_string_lossy()
                .replace('\\', "/");
            snapshot.insert(relative, fingerprint(&metadata));
        }
        Ok(snapshot)
    })
    .await
    .map_err(|error| BridgeError::tool("watch_failed", error.to_string()))?
}

fn watch_event_path(record: &WatchRecord, relative: &str) -> String {
    public_path(
        &record.grant_id,
        &record.grant_relative.join(relative.replace('/', "\\")),
    )
}

async fn refresh_watch(state: &AppState, watch_id: &str) -> Result<(), BridgeError> {
    let (path, recursive) = {
        let watches = state.watches.lock().await;
        let record = watches
            .get(watch_id)
            .ok_or_else(|| BridgeError::tool("watch_not_found", "watch not found"))?;
        if record.closed {
            return Ok(());
        }
        (record.real_path.clone(), record.recursive)
    };
    let next = snapshot_path(path, recursive).await?;
    let mut watches = state.watches.lock().await;
    let record = watches
        .get_mut(watch_id)
        .ok_or_else(|| BridgeError::tool("watch_not_found", "watch not found"))?;
    let mut changes = Vec::new();
    for (path, fingerprint) in &next {
        match record.snapshot.get(path) {
            None => changes.push(("rename", path.clone())),
            Some(previous) if previous != fingerprint => changes.push(("change", path.clone())),
            _ => {}
        }
    }
    for path in record.snapshot.keys() {
        if !next.contains_key(path) {
            changes.push(("rename", path.clone()));
        }
    }
    changes.sort_by(|left, right| left.1.cmp(&right.1));
    for (event_type, relative_path) in changes {
        record.sequence += 1;
        let event_path = watch_event_path(record, &relative_path);
        let sequence = record.sequence;
        record.events.push_back(json!({
            "sequence": sequence,
            "eventType": event_type,
            "relativePath": if relative_path.is_empty() { Value::Null } else { Value::String(relative_path) },
            "path": event_path,
            "timestamp": chrono::Utc::now().to_rfc3339()
        }));
        while record.events.len() > 1000 {
            record.events.pop_front();
        }
    }
    record.snapshot = next;
    Ok(())
}

async fn start_watch(state: &AppState, arguments: &Value) -> Result<Value, BridgeError> {
    let resolved = state
        .resolver
        .resolve(argument_path(arguments)?, AccessMode::Read, false)?;
    let recursive = arguments
        .get("recursive")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if recursive && !resolved.real.is_dir() {
        return Err(BridgeError::tool(
            "watch_directory_required",
            "recursive watch requires a directory",
        ));
    }
    let id = Uuid::new_v4().to_string();
    let created_at = chrono::Utc::now().to_rfc3339();
    let record = WatchRecord {
        id: id.clone(),
        public_path: public_path(&resolved.grant.id, &resolved.relative),
        real_path: resolved.real.clone(),
        grant_id: resolved.grant.id.clone(),
        grant_relative: resolved.relative.clone(),
        recursive,
        created_at: created_at.clone(),
        sequence: 0,
        events: VecDeque::new(),
        snapshot: snapshot_path(resolved.real, recursive).await?,
        closed: false,
    };
    state.watches.lock().await.insert(id.clone(), record);
    Ok(content_json(json!({
        "watchId": id,
        "path": public_path(&resolved.grant.id, &resolved.relative),
        "recursive": recursive,
        "debounceMs": argument_usize(arguments, "debounceMs", 150, 0, 5000),
        "createdAt": created_at
    })))
}

async fn watch_events(state: &AppState, arguments: &Value) -> Result<Value, BridgeError> {
    let watch_id = argument_string(arguments, "watchId", "watch_not_found")?;
    let after_sequence = arguments
        .get("afterSequence")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let limit = argument_usize(arguments, "limit", 100, 1, 1000);
    let wait_ms = argument_usize(arguments, "waitMs", 0, 0, 30_000);
    let deadline = std::time::Instant::now() + Duration::from_millis(wait_ms as u64);
    loop {
        refresh_watch(state, watch_id).await?;
        let result = {
            let watches = state.watches.lock().await;
            let record = watches
                .get(watch_id)
                .ok_or_else(|| BridgeError::tool("watch_not_found", "watch not found"))?;
            let events = record
                .events
                .iter()
                .filter(|event| {
                    event
                        .get("sequence")
                        .and_then(Value::as_u64)
                        .is_some_and(|sequence| sequence > after_sequence)
                })
                .take(limit)
                .cloned()
                .collect::<Vec<_>>();
            let oldest_sequence = record
                .events
                .front()
                .and_then(|event| event.get("sequence"))
                .and_then(Value::as_u64)
                .unwrap_or(record.sequence + 1);
            json!({
                "watchId": record.id,
                "path": record.public_path,
                "recursive": record.recursive,
                "createdAt": record.created_at,
                "closed": record.closed,
                "afterSequence": after_sequence,
                "currentSequence": record.sequence,
                "oldestSequence": oldest_sequence,
                "overflowed": after_sequence > 0 && after_sequence < oldest_sequence.saturating_sub(1),
                "hasMore": events.last().and_then(|event| event.get("sequence")).and_then(Value::as_u64).is_some_and(|sequence| sequence < record.sequence),
                "events": events
            })
        };
        if result["events"]
            .as_array()
            .is_some_and(|events| !events.is_empty())
            || wait_ms == 0
            || std::time::Instant::now() >= deadline
        {
            return Ok(content_json(result));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn stop_watch(state: &AppState, arguments: &Value) -> Result<Value, BridgeError> {
    let watch_id = argument_string(arguments, "watchId", "watch_not_found")?;
    let removed = state.watches.lock().await.remove(watch_id);
    if removed.is_none() {
        return Err(BridgeError::tool("watch_not_found", "watch not found"));
    }
    Ok(content_json(json!({
        "watchId": watch_id,
        "closed": true
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utf16le_bom(text: &str) -> Vec<u8> {
        let mut bytes = vec![0xff, 0xfe];
        for unit in text.encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        bytes
    }

    #[test]
    fn decode_search_text_handles_utf8_bom_and_utf16() {
        assert_eq!(decode_search_text(b"plain").as_deref(), Some("plain"));
        let mut utf8_bom = vec![0xef, 0xbb, 0xbf];
        utf8_bom.extend_from_slice(b"hello");
        assert_eq!(decode_search_text(&utf8_bom).as_deref(), Some("hello"));
        let utf16 = utf16le_bom("你好 world");
        assert_eq!(decode_search_text(&utf16).as_deref(), Some("你好 world"));
    }

    #[test]
    fn decode_search_text_rejects_binary() {
        // Realistic binary: dense NULs plus scattered high bytes.
        let mut binary = Vec::with_capacity(4096);
        for index in 0..4096 {
            binary.push(if index % 3 == 0 {
                0
            } else {
                (index % 251) as u8
            });
        }
        assert_eq!(decode_search_text(&binary), None);
    }

    #[test]
    fn decode_search_text_handles_bomless_utf16le() {
        // 'A\0B\0' pattern without BOM
        let bytes = b"A\0B\0C\0D\0E\0";
        assert_eq!(decode_search_text(bytes).as_deref(), Some("ABCDE"));
    }

    #[test]
    fn content_hits_matches_needle_and_regex() {
        let text = "line one\nfind KEY here\nline three\nKEY again\n";
        let hits = content_hits(text, Some("key"), None, 10);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0]["line"], 2);
        assert!(
            hits[0]["snippet"]
                .as_str()
                .unwrap()
                .contains("find KEY here")
        );

        let regex = regex::Regex::new(r"KEY \w+").unwrap();
        let hits = content_hits(text, None, Some(&regex), 10);
        assert_eq!(hits.len(), 2);

        // AND semantics: needle + regex both required
        let hits = content_hits(text, Some("again"), Some(&regex), 10);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0]["line"], 4);
    }

    #[test]
    fn content_hits_truncates_long_snippets_and_bounds_matches() {
        let long_line = "x".repeat(500);
        let text = format!("{long_line}\n{long_line}\n");
        let hits = content_hits(&text, Some("x"), None, 1);
        assert_eq!(hits.len(), 1);
        let snippet = hits[0]["snippet"].as_str().unwrap();
        assert!(snippet.ends_with('…'));
        assert!(snippet.chars().count() <= 241);
    }

    #[test]
    fn content_hits_handles_crlf() {
        let text = "first\r\nsecond\r\n";
        let hits = content_hits(text, Some("second"), None, 10);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0]["line"], 2);
        assert_eq!(hits[0]["snippet"], "second");
    }
}

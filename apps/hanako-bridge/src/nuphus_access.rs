//! Nuphus 桌面/浏览器自动化能力授权 — 将 nuphus 自动化工具纳入文件桥授权体系。
//!
//! 只读工具(截图/列表/查询)默认允许;写工具(鼠标/键盘/窗口控制/剪贴板写/
//! 浏览器操作)必须持有对应 capability 的活跃授权,授权通过 chat quote
//! 校验获取(与文件/执行授权同源),持久化在 `data/nuphus-access.json`。

use std::path::PathBuf;
use std::sync::Mutex;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use hanako_bridge_core::{
    BridgeError,
    path::{quote_contains_negation, quote_contains_token},
};

const NUPHUS_ACCESS_FILE: &str = "nuphus-access.json";

/// 能力组:写工具按风险域分组授权。
pub const CAP_DESKTOP_CONTROL: &str = "desktop.control";
pub const CAP_DESKTOP_INPUT: &str = "desktop.input";
pub const CAP_DESKTOP_WINDOW: &str = "desktop.window";
pub const CAP_DESKTOP_CLIPBOARD: &str = "desktop.clipboard";
pub const CAP_BROWSER_CONTROL: &str = "browser.control";

pub const ALL_CAPABILITIES: &[&str] = &[
    CAP_DESKTOP_CONTROL,
    CAP_DESKTOP_INPUT,
    CAP_DESKTOP_WINDOW,
    CAP_DESKTOP_CLIPBOARD,
    CAP_BROWSER_CONTROL,
];

/// 写工具 → 能力组(与 `nuphus_mcp_core::security::is_write_tool` 配合:
/// 只读放行,写工具按此分组)。
fn capability_group_for(name: &str) -> &'static str {
    if name.starts_with("browser_") {
        CAP_BROWSER_CONTROL
    } else if matches!(
        name,
        "desktop_mouse" | "desktop_mouse_drag" | "desktop_input"
    ) {
        CAP_DESKTOP_INPUT
    } else if matches!(
        name,
        "desktop_window_activate" | "desktop_window_move" | "desktop_window_resize"
    ) {
        CAP_DESKTOP_WINDOW
    } else if matches!(name, "desktop_clipboard_write" | "desktop_clipboard_clean") {
        CAP_DESKTOP_CLIPBOARD
    } else {
        CAP_DESKTOP_CONTROL
    }
}

/// capability 域词表:quote 必须命中对应域,防止"允许访问 C:\data"这类
/// 文件授权被挪用来授权桌面控制。
fn capability_domain_terms(capability: &str) -> &'static [&'static str] {
    match capability {
        CAP_DESKTOP_INPUT => &[
            "鼠标", "键盘", "点击", "输入", "mouse", "keyboard", "click", "type", "键入",
        ],
        CAP_DESKTOP_WINDOW => &["窗口", "window", "应用窗口", "移动窗口", "调整窗口"],
        CAP_DESKTOP_CLIPBOARD => &["剪贴板", "clipboard", "复制", "粘贴"],
        CAP_BROWSER_CONTROL => &[
            "浏览器",
            "网页",
            "chrome",
            "browser",
            "web",
            "页面",
            "标签页",
            "上网",
        ],
        _ => &[
            "桌面",
            "屏幕",
            "desktop",
            "screen",
            "界面",
            "gui",
            "automation",
            "自动化",
            "操作系统",
            "操控",
        ],
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NuphusGrant {
    pub id: String,
    pub capability: String,
    pub source: String,
    pub quote: String,
    pub created_at: String,
    pub expires_at: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct NuphusStore {
    schema_version: u32,
    grants: Vec<NuphusGrant>,
}

pub struct NuphusAccess {
    path: PathBuf,
    store: Mutex<NuphusStore>,
}

fn store_error(message: impl Into<String>) -> BridgeError {
    BridgeError::tool("nuphus_store_error", message)
}

impl NuphusAccess {
    pub async fn new(data_dir: PathBuf) -> anyhow::Result<Self> {
        let path = data_dir.join(NUPHUS_ACCESS_FILE);
        let store = match tokio::fs::read(&path).await {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|_| NuphusStore {
                schema_version: 1,
                grants: Vec::new(),
            }),
            Err(_) => NuphusStore {
                schema_version: 1,
                grants: Vec::new(),
            },
        };
        Ok(Self {
            path,
            store: Mutex::new(store),
        })
    }

    fn persist(&self) -> Result<(), BridgeError> {
        let store = self
            .store
            .lock()
            .map_err(|_| store_error("nuphus access lock poisoned"))?;
        let bytes = serde_json::to_vec_pretty(&*store)
            .map_err(|error| store_error(format!("serialize: {error}")))?;
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| store_error(format!("create data dir: {error}")))?;
        }
        std::fs::write(&self.path, bytes)
            .map_err(|error| store_error(format!("write {}: {error}", self.path.display())))
    }

    pub fn list_grants(&self) -> Vec<NuphusGrant> {
        self.store
            .lock()
            .map(|store| store.grants.clone())
            .unwrap_or_default()
    }

    /// 校验 quote 并创建 capability 授权(与文件/执行 chat 授权同源)。
    pub fn request(
        &self,
        capability: &str,
        quote: &str,
        chat_grant_minutes: u64,
    ) -> Result<NuphusGrant, BridgeError> {
        if !ALL_CAPABILITIES.contains(&capability) {
            return Err(BridgeError::tool(
                "unknown_capability",
                format!(
                    "unknown capability '{capability}'; expected one of: {}",
                    ALL_CAPABILITIES.join(", ")
                ),
            ));
        }
        validate_capability_quote(capability, quote)?;
        let now = Utc::now();
        let grant = NuphusGrant {
            id: Uuid::new_v4().to_string(),
            capability: capability.to_string(),
            source: "chat_authorization".to_string(),
            quote: quote.trim().to_string(),
            created_at: now.to_rfc3339(),
            expires_at: Some(
                (now + chrono::Duration::minutes(chat_grant_minutes as i64)).to_rfc3339(),
            ),
        };
        self.store
            .lock()
            .map_err(|_| store_error("nuphus access lock poisoned"))?
            .grants
            .push(grant.clone());
        self.persist()?;
        Ok(grant)
    }

    /// 工具执行前的授权门:只读工具放行,写工具检查对应 capability。
    pub fn require(&self, name: &str, args: &serde_json::Value) -> Result<(), BridgeError> {
        if !nuphus_mcp_core::security::is_write_tool(name, args) {
            return Ok(());
        }
        let capability = capability_group_for(name);
        self.check(capability)
    }

    /// fail-closed 检查:capability 必须有活跃授权(未过期、解析失败视为过期)。
    pub fn check(&self, capability: &str) -> Result<(), BridgeError> {
        let now = Utc::now();
        let active = self
            .store
            .lock()
            .map_err(|_| store_error("nuphus access lock poisoned"))?
            .grants
            .iter()
            .any(|grant| {
                grant.capability == capability
                    && match grant.expires_at.as_deref() {
                        None => true,
                        Some(value) => DateTime::parse_from_rfc3339(value)
                            .map(|expires| expires > now)
                            .unwrap_or(false),
                    }
            });
        if active {
            Ok(())
        } else {
            Err(BridgeError::tool(
                "nuphus_capability_not_authorized",
                format!(
                    "nuphus automation capability '{capability}' is not authorized. \
                     Ask the user for an explicit authorization message mentioning \
                     the capability (e.g. \"允许操作桌面/鼠标/浏览器\") and call \
                     nuphus.request_access with that quote."
                ),
            ))
        }
    }

    pub fn revoke(&self, grant_id: &str) -> Result<(), BridgeError> {
        let mut store = self
            .store
            .lock()
            .map_err(|_| store_error("nuphus access lock poisoned"))?;
        let before = store.grants.len();
        store.grants.retain(|grant| grant.id != grant_id);
        if store.grants.len() == before {
            return Err(BridgeError::tool("grant_not_found", "grant not found"));
        }
        drop(store);
        self.persist()
    }
}

fn validate_capability_quote(capability: &str, quote: &str) -> Result<(), BridgeError> {
    let quote = quote.trim();
    if !(8..=2000).contains(&quote.len()) {
        return Err(BridgeError::tool(
            "explicit_authorization_required",
            "the exact current user authorization message is required",
        ));
    }
    if quote_contains_negation(quote) {
        return Err(BridgeError::tool(
            "explicit_authorization_required",
            "the user message must not contain a negation and must explicitly authorize access",
        ));
    }
    let lower = quote.to_ascii_lowercase();
    let authorized_zh = ["授权", "允许", "同意", "批准", "准许"]
        .iter()
        .any(|word| lower.contains(word));
    let authorized_en = [
        "authorize",
        "authorized",
        "authorizing",
        "allow",
        "allowed",
        "allowing",
        "approve",
        "approved",
        "approves",
        "permission",
        "permit",
        "permits",
        "permitted",
    ]
    .iter()
    .any(|word| quote_contains_token(&lower, word));
    if !(authorized_zh || authorized_en) {
        return Err(BridgeError::tool(
            "explicit_authorization_required",
            "the user message must explicitly authorize access",
        ));
    }
    // 域词:quote 必须提到对应能力域(桌面/鼠标/键盘/窗口/浏览器等)。
    let domain_hit = capability_domain_terms(capability)
        .iter()
        .any(|term| lower.contains(&term.to_ascii_lowercase()));
    if !domain_hit {
        return Err(BridgeError::tool(
            "capability_domain_not_confirmed",
            format!(
                "the user message must mention the '{capability}' capability domain \
                 (e.g. 桌面/鼠标/键盘/窗口/浏览器/剪贴板)"
            ),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn temp_access() -> (tempfile_dir::TempDir, NuphusAccess) {
        let dir = tempfile_dir::TempDir::new().unwrap();
        let access = NuphusAccess::new(dir.path().to_path_buf())
            .await
            .expect("create access");
        (dir, access)
    }

    #[tokio::test]
    async fn request_requires_explicit_authorization() {
        let (_dir, access) = temp_access().await;
        // 否定句拒绝。
        let err = access
            .request(CAP_DESKTOP_INPUT, "不允许你操作我的鼠标", 30)
            .unwrap_err();
        assert_eq!(err.code(), "explicit_authorization_required");
        // 缺授权词拒绝。
        let err = access
            .request(CAP_DESKTOP_INPUT, "今天天气不错适合散步", 30)
            .unwrap_err();
        assert_eq!(err.code(), "explicit_authorization_required");
        // 授权词但缺能力域词拒绝(文件授权不能挪用于桌面控制)。
        let err = access
            .request(CAP_DESKTOP_INPUT, "我允许你访问 C:\\data 目录", 30)
            .unwrap_err();
        assert_eq!(err.code(), "capability_domain_not_confirmed");
        // 合法授权成功。
        let grant = access
            .request(CAP_DESKTOP_INPUT, "我允许你操作我的鼠标和键盘", 30)
            .expect("grant");
        assert_eq!(grant.capability, CAP_DESKTOP_INPUT);
        assert!(grant.expires_at.is_some());
    }

    #[tokio::test]
    async fn check_is_fail_closed_and_expires() {
        let (_dir, access) = temp_access().await;
        // 未授权 → 拒绝。
        assert!(access.check(CAP_BROWSER_CONTROL).is_err());
        // 授权后 → 放行。
        access
            .request(CAP_BROWSER_CONTROL, "我同意你操作我的浏览器", 30)
            .expect("grant");
        assert!(access.check(CAP_BROWSER_CONTROL).is_ok());
        // 不同 capability 互不影响。
        assert!(access.check(CAP_DESKTOP_INPUT).is_err());
        // 已过期 → 拒绝(构造过期 grant)。
        access.store.lock().unwrap().grants.push(NuphusGrant {
            id: "expired".to_string(),
            capability: CAP_DESKTOP_INPUT.to_string(),
            source: "test".to_string(),
            quote: "test".to_string(),
            created_at: Utc::now().to_rfc3339(),
            expires_at: Some((Utc::now() - chrono::Duration::minutes(1)).to_rfc3339()),
        });
        assert!(access.check(CAP_DESKTOP_INPUT).is_err());
    }

    #[tokio::test]
    async fn require_gates_write_tools_only() {
        let (_dir, access) = temp_access().await;
        // 只读工具无授权直接放行。
        let args = serde_json::json!({});
        assert!(access.require("desktop_screen_size", &args).is_ok());
        assert!(access.require("browser_snapshot", &args).is_ok());
        // desktop_mouse 默认 action=position(只读查询)也放行。
        assert!(access.require("desktop_mouse", &args).is_ok());
        // 写操作(click)无授权 → 拒绝。
        assert!(
            access
                .require("desktop_mouse", &serde_json::json!({ "action": "click" }))
                .is_err()
        );
        assert!(access.require("browser_click", &args).is_err());
        // 授权对应组后放行。
        access
            .request(CAP_DESKTOP_INPUT, "我允许你操作鼠标键盘", 30)
            .expect("grant");
        assert!(
            access
                .require("desktop_mouse", &serde_json::json!({ "action": "click" }))
                .is_ok()
        );
        assert!(access.require("browser_click", &args).is_err());
    }

    #[tokio::test]
    async fn revoke_removes_grant() {
        let (_dir, access) = temp_access().await;
        let grant = access
            .request(CAP_DESKTOP_CLIPBOARD, "我允许你使用剪贴板", 30)
            .expect("grant");
        assert!(access.check(CAP_DESKTOP_CLIPBOARD).is_ok());
        access.revoke(&grant.id).expect("revoke");
        assert!(access.check(CAP_DESKTOP_CLIPBOARD).is_err());
        assert!(access.revoke(&grant.id).is_err());
    }
}

/// 轻量临时目录(避免引入 tempfile 依赖)。
#[cfg(test)]
mod tempfile_dir {
    use std::path::{Path, PathBuf};

    pub struct TempDir(PathBuf);

    impl TempDir {
        pub fn new() -> std::io::Result<Self> {
            let path =
                std::env::temp_dir().join(format!("nuphus-access-test-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&path)?;
            Ok(Self(path))
        }

        pub fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

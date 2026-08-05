use std::{
    path::{Component, Path, PathBuf},
    sync::{Arc, RwLock},
};

use serde::Serialize;

use crate::{
    BridgeError, BridgeResult,
    config::{RootConfig, RootMode},
};

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Grant {
    pub id: String,
    pub name: String,
    pub path: PathBuf,
    pub mode: RootMode,
    pub enabled: bool,
    pub source: String,
}

#[derive(Clone, Debug)]
pub struct ResolvedPath {
    pub grant: Arc<Grant>,
    pub real: PathBuf,
    pub relative: PathBuf,
    pub exists: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccessMode {
    Read,
    ReadWrite,
}

#[derive(Clone)]
pub struct PathResolver {
    device_id: String,
    aliases: Vec<String>,
    full_trust: bool,
    grants: Arc<RwLock<Vec<Arc<Grant>>>>,
}

fn normalize_for_compare(path: &Path) -> String {
    path.to_string_lossy()
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_ascii_lowercase()
}

fn is_inside(path: &Path, root: &Path) -> bool {
    let Ok(path) = normalize_absolute_local(path) else {
        return false;
    };
    let Ok(root) = normalize_absolute_local(root) else {
        return false;
    };
    let path = normalize_for_compare(&path);
    let root = normalize_for_compare(&root);
    path == root || path.starts_with(&(root + "\\"))
}

fn clean_grant_name(value: &str) -> String {
    let mut output = String::new();
    let mut separator = false;
    for character in value.trim().chars() {
        if character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-') {
            output.push(character);
            separator = false;
        } else if !separator && !output.is_empty() {
            output.push('-');
            separator = true;
        }
        if output.len() >= 64 {
            break;
        }
    }
    output.trim_matches('-').to_string()
}

fn normalize_device_path(input: &str, device_id: &str, aliases: &[String]) -> BridgeResult<String> {
    let Some(rest) = input.strip_prefix("device://") else {
        return Ok(input.to_string());
    };
    let Some((requested, path)) = rest.split_once('/') else {
        return Err(BridgeError::tool(
            "invalid_local_path",
            "device path is missing an absolute path",
        ));
    };
    let requested = requested.to_ascii_lowercase();
    let accepted = std::iter::once(device_id)
        .chain(aliases.iter().map(String::as_str))
        .any(|value| value.to_ascii_lowercase() == requested);
    if !accepted {
        return Err(BridgeError::tool(
            "wrong_device",
            format!("path targets device {requested}, but this bridge is {device_id}"),
        ));
    }
    Ok(path.to_string())
}

fn validate_absolute_local_path(path: &Path) -> BridgeResult<()> {
    let text = path.to_string_lossy();
    let bytes = text.as_bytes();
    if bytes.len() < 3
        || !bytes[0].is_ascii_alphabetic()
        || bytes[1] != b':'
        || !matches!(bytes[2], b'\\' | b'/')
    {
        return Err(BridgeError::tool(
            "invalid_local_path",
            "only absolute local Windows drive paths are allowed",
        ));
    }
    if text.contains('\0') || text.starts_with(r"\\.\") || text.starts_with(r"\\?\") {
        return Err(BridgeError::tool(
            "invalid_local_path",
            "device paths are not allowed",
        ));
    }
    if text[2..].contains(':') {
        return Err(BridgeError::tool(
            "invalid_local_path",
            "alternate data streams are not allowed",
        ));
    }
    Ok(())
}

/// 规范化绝对 Windows 本地路径:消费 `.`/`..` 组件,阻止逃逸盘根。
///
/// Windows 文件 API 会真实解析 `..`,而字符串前缀比较不会,因此任何基于
/// 前缀的授权判断必须先经过本函数,否则 `C:\root\..\..\secret` 一类输入
/// 会绕过授权根目录检查。
pub fn normalize_absolute_local(path: &Path) -> BridgeResult<PathBuf> {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::Normal(value) => normalized.push(value),
            Component::ParentDir => {
                // 盘根(如 `C:\`)上的 `..` 是根目录自身,忽略即可;
                // 空路径上的 `..` 才视为逃逸,拒绝。
                if !normalized.pop() && !normalized.has_root() {
                    return Err(BridgeError::tool(
                        "invalid_local_path",
                        "path escapes the drive root",
                    ));
                }
            }
        }
    }
    Ok(normalized)
}

fn normalize_relative(path: &Path) -> BridgeResult<PathBuf> {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(value) => normalized.push(value),
            Component::ParentDir => {
                if !normalized.pop() {
                    return Err(BridgeError::tool(
                        "path_outside_root",
                        "path escapes the authorized root",
                    ));
                }
            }
            Component::Prefix(_) | Component::RootDir => {
                return Err(BridgeError::tool(
                    "invalid_local_path",
                    "local:// paths must contain a relative suffix",
                ));
            }
        }
    }
    Ok(normalized)
}

impl PathResolver {
    pub fn new(
        roots: &[RootConfig],
        full_trust: bool,
        device_id: impl Into<String>,
        aliases: Vec<String>,
    ) -> Self {
        let grants = roots
            .iter()
            .map(|root| {
                Arc::new(Grant {
                    id: {
                        let cleaned = clean_grant_name(&root.name);
                        if cleaned.is_empty() {
                            "LocalFiles".to_string()
                        } else {
                            cleaned
                        }
                    },
                    name: root.name.clone(),
                    path: root.path.clone(),
                    mode: root.mode,
                    enabled: true,
                    source: "bootstrap".to_string(),
                })
            })
            .collect();
        Self {
            device_id: device_id.into().to_ascii_lowercase(),
            aliases,
            full_trust,
            grants: Arc::new(RwLock::new(grants)),
        }
    }

    pub fn grants(&self) -> Vec<Grant> {
        self.grants
            .read()
            .expect("path grant lock is not poisoned")
            .iter()
            .map(|grant| (**grant).clone())
            .collect()
    }

    pub fn replace_grants(&self, grants: Vec<Grant>) {
        *self
            .grants
            .write()
            .expect("path grant lock is not poisoned") = grants.into_iter().map(Arc::new).collect();
    }

    pub fn resolve(
        &self,
        input: &str,
        mode: AccessMode,
        allow_missing: bool,
    ) -> BridgeResult<ResolvedPath> {
        let normalized = normalize_device_path(input.trim(), &self.device_id, &self.aliases)?;
        if let Some(rest) = normalized.strip_prefix("local://") {
            let (grant_id, relative) = rest.split_once('/').unwrap_or((rest, ""));
            let grant = self
                .grants
                .read()
                .expect("path grant lock is not poisoned")
                .iter()
                .find(|grant| grant.id.eq_ignore_ascii_case(grant_id))
                .cloned()
                .ok_or_else(|| BridgeError::tool("grant_not_found", "authorized root not found"))?;
            if mode == AccessMode::ReadWrite
                && grant.mode != RootMode::ReadWrite
                && !self.full_trust
            {
                return Err(BridgeError::tool(
                    "write_not_authorized",
                    "authorized root is read-only",
                ));
            }
            let relative = normalize_relative(Path::new(relative))?;
            let real = grant.path.join(&relative);
            if !is_inside(&real, &grant.path) {
                return Err(BridgeError::tool(
                    "path_outside_root",
                    "path escapes the authorized root",
                ));
            }
            let exists = real.exists();
            if !exists && !allow_missing {
                return Err(BridgeError::tool("path_not_found", "path does not exist"));
            }
            return Ok(ResolvedPath {
                grant,
                real,
                relative,
                exists,
            });
        }

        let absolute = PathBuf::from(normalized);
        validate_absolute_local_path(&absolute)?;
        let absolute = normalize_absolute_local(&absolute)?;
        if self.full_trust {
            let root = PathBuf::from(format!(
                "{}\\",
                absolute
                    .to_string_lossy()
                    .chars()
                    .take(2)
                    .collect::<String>()
            ));
            let grant = Arc::new(Grant {
                id: format!(
                    "drive-{}",
                    absolute
                        .to_string_lossy()
                        .chars()
                        .next()
                        .unwrap_or('c')
                        .to_ascii_lowercase()
                ),
                name: root.to_string_lossy().to_string(),
                path: root.clone(),
                mode: RootMode::ReadWrite,
                enabled: true,
                source: "full_trust".to_string(),
            });
            let relative = absolute
                .strip_prefix(&root)
                .map(Path::to_path_buf)
                .unwrap_or_default();
            let exists = absolute.exists();
            if !exists && !allow_missing {
                return Err(BridgeError::tool("path_not_found", "path does not exist"));
            }
            return Ok(ResolvedPath {
                grant,
                real: absolute,
                relative,
                exists,
            });
        }

        let grants = self.grants.read().expect("path grant lock is not poisoned");
        for grant in grants.iter() {
            if !is_inside(&absolute, &grant.path) {
                continue;
            }
            if mode == AccessMode::ReadWrite && grant.mode != RootMode::ReadWrite {
                return Err(BridgeError::tool(
                    "write_not_authorized",
                    "authorized root is read-only",
                ));
            }
            let relative = absolute
                .strip_prefix(&grant.path)
                .map(Path::to_path_buf)
                .unwrap_or_default();
            let exists = absolute.exists();
            if !exists && !allow_missing {
                return Err(BridgeError::tool("path_not_found", "path does not exist"));
            }
            return Ok(ResolvedPath {
                grant: Arc::clone(grant),
                real: absolute,
                relative,
                exists,
            });
        }
        Err(BridgeError::tool(
            "access_denied",
            "path is outside the authorized roots",
        ))
    }
}

pub fn public_path(grant_id: &str, relative: &Path) -> String {
    let suffix = relative.to_string_lossy().replace('\\', "/");
    if suffix.is_empty() {
        format!("local://{grant_id}")
    } else {
        format!("local://{grant_id}/{suffix}")
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn resolves_local_alias_and_blocks_parent_escape() {
        let dir = tempdir().unwrap();
        let resolver = PathResolver::new(
            &[RootConfig {
                name: "Root".to_string(),
                path: dir.path().to_path_buf(),
                mode: RootMode::ReadWrite,
            }],
            false,
            "test-device",
            Vec::new(),
        );

        let resolved = resolver
            .resolve("local://Root/child.txt", AccessMode::ReadWrite, true)
            .unwrap();
        assert_eq!(resolved.real, dir.path().join("child.txt"));
        assert!(
            resolver
                .resolve("local://Root/../../escape.txt", AccessMode::Read, true)
                .is_err()
        );
    }

    #[test]
    fn blocks_parent_escape_in_absolute_paths() {
        let dir = tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let resolver = PathResolver::new(
            &[RootConfig {
                name: "Root".to_string(),
                path: root.clone(),
                mode: RootMode::ReadWrite,
            }],
            false,
            "test-device",
            Vec::new(),
        );

        // `..` 逃逸授权根必须被拒绝,即使字符串前缀看起来在根内。
        let escape = format!("{}\\..\\..\\Windows\\System32\\config\\SAM", root.display());
        assert!(
            resolver.resolve(&escape, AccessMode::Read, true).is_err(),
            "absolute path with .. must not escape the authorized root"
        );

        // 根内路径带 `..` 应规范化为真实路径。
        let inside = format!("{}\\child\\..\\note.txt", root.display());
        let resolved = resolver
            .resolve(&inside, AccessMode::ReadWrite, true)
            .unwrap();
        assert_eq!(resolved.real, root.join("note.txt"));
    }

    #[test]
    fn normalize_absolute_local_consumes_dot_dot() {
        let normalized = normalize_absolute_local(Path::new(r"C:\data\..\secret.txt")).unwrap();
        assert_eq!(normalized, PathBuf::from(r"C:\secret.txt"));

        let normalized = normalize_absolute_local(Path::new(r"C:\data\child\..\..\x.txt")).unwrap();
        assert_eq!(normalized, PathBuf::from(r"C:\x.txt"));

        // 根目录的 `..` 即根目录自身,规范化后不逃逸。
        let normalized = normalize_absolute_local(Path::new(r"C:\..\secret.txt")).unwrap();
        assert_eq!(normalized, PathBuf::from(r"C:\secret.txt"));

        // 盘符相对路径(C:xxx)不属于绝对路径,由调用方在 validate 层拦截。
        let _ = normalize_absolute_local(Path::new(r"C:relative.txt")).unwrap();
    }
}

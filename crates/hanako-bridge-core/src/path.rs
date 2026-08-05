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

/// 判断授权原话中是否出现指定路径(大小写不敏感,`/` 与 `\` 等价),
/// 并要求匹配位置前后都不是路径组成部分字符,避免 `C:\data` 误匹配
/// `C:\data2` 或 `C:\data-archive` 这类相邻路径。
pub fn quote_contains_path(quote: &str, path: &str) -> bool {
    bounded_contains(
        &quote.replace('/', "\\").to_ascii_lowercase(),
        &path.replace('/', "\\").to_ascii_lowercase(),
    )
}

/// 大小写敏感的边界子串匹配(用于命令参数等区分大小写的 token),
/// 避免短参数如 `-f` 匹配到 `config-file` 内部。
pub fn quote_contains_token(quote: &str, token: &str) -> bool {
    bounded_contains(quote, token)
}

/// 判断授权原话是否包含否定表述。
///
/// 授权词检查用的是子串匹配,"不要授权"/"don't allow" 会命中授权词,
/// 导致用户明确拒绝的消息被当作显式授权。本函数检出常见中英文否定词
/// (含 can't/won't/nothing 及 denied/refused 等词形变化),命中即视为
/// 未授权(fail-closed);中文词用 contains,英文词用独立单词边界匹配,
/// 标点(! . , 等)对英文单词是边界,字母数字才是延续。
pub fn quote_contains_negation(quote: &str) -> bool {
    let lower = quote.to_ascii_lowercase();
    const NEGATION_ZH: &[&str] = &[
        "不",
        "未",
        "未经",
        "尚未",
        "无",
        "从不",
        "不要",
        "不能",
        "不可以",
        "不准",
        "不许",
        "拒绝",
        "禁止",
        "严禁",
        "切勿",
        "不得",
        "勿",
        "别",
        "没有",
        "别让",
        "别把",
        "不同意",
        "无法",
        "不允许",
        "绝不",
    ];
    const NEGATION_EN: &[&str] = &[
        "don't",
        "dont",
        "can't",
        "cant",
        "won't",
        "wont",
        "wouldn't",
        "couldn't",
        "shouldn't",
        "isn't",
        "aren't",
        "haven't",
        "doesn't",
        "didn't",
        "mustn't",
        "no",
        "not",
        "nothing",
        "nobody",
        "nowhere",
        "never",
        "without",
        "unless",
        "except",
        "withhold",
        "deny",
        "denied",
        "denying",
        "denies",
        "refuse",
        "refused",
        "refusing",
        "prohibit",
        "prohibited",
        "prohibits",
        "forbid",
        "forbidden",
        "forbids",
        "forbade",
        "cannot",
        "unable",
        "decline",
        "declined",
        "reject",
        "rejected",
    ];
    NEGATION_ZH.iter().any(|word| lower.contains(word))
        || lower.contains("n't")
        || NEGATION_EN
            .iter()
            .any(|word| bounded_contains_word(&lower, word))
}

/// 英文否定词的边界匹配:匹配位置前后必须是"非字母数字"字符。
///
/// 与路径延续字符集不同——标点(`!` `.` `,` 等)对英文单词来说是边界,
/// `"No! I allow..."`、`"I give no. permission..."` 中的 `no` 都应命中,
/// 而 `node`/`note` 内部不算(`nothing` 由独立词条覆盖)。
fn bounded_contains_word(haystack: &str, word: &str) -> bool {
    if word.is_empty() {
        return true;
    }
    let bytes = haystack.as_bytes();
    let needle = word.as_bytes();
    if needle.len() > bytes.len() {
        return false;
    }
    let mut index = 0;
    while index + needle.len() <= bytes.len() {
        let Some(relative) = bytes[index..]
            .windows(needle.len())
            .position(|window| window == needle)
        else {
            break;
        };
        let position = index + relative;
        let end = position + needle.len();
        let before_ok = position == 0 || !bytes[position - 1].is_ascii_alphanumeric();
        let after_ok = end >= bytes.len() || !bytes[end].is_ascii_alphanumeric();
        if before_ok && after_ok {
            return true;
        }
        index = position + 1;
    }
    false
}

fn bounded_contains(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    let haystack = haystack.as_bytes();
    let needle = needle.as_bytes();
    if needle.len() > haystack.len() {
        return false;
    }
    let mut index = 0;
    while index + needle.len() <= haystack.len() {
        let Some(relative) = haystack[index..]
            .windows(needle.len())
            .position(|window| window == needle)
        else {
            break;
        };
        let position = index + relative;
        let end = position + needle.len();
        let before_ok = position == 0 || !is_path_continuation_before(haystack, position);
        let after_ok = end >= haystack.len() || !is_path_continuation_after(haystack, end);
        if before_ok && after_ok {
            return true;
        }
        index = position + 1;
    }
    false
}

/// 匹配位置前一个字符是否属于路径延续。
///
/// ASCII 用延续字符集(不含括号,括号包裹路径如 "(C:\data)" 是常见
/// 聊天表达);非 ASCII(如中文动词"授权C:\data"中紧贴路径的汉字)
/// 不视为延续——路径以盘符开头,前面的字符不可能构成更长路径名。
fn is_path_continuation_before(bytes: &[u8], index: usize) -> bool {
    let byte = bytes[index - 1];
    if byte.is_ascii() {
        is_ascii_continuation(byte)
    } else {
        false
    }
}

/// 匹配位置后一个字符是否属于路径延续。
///
/// 与 before 不同,after 侧 `(` 视为延续,拒绝 "C:\data(1)" 这类
/// 常见命名被 `C:\data` 误匹配;`)` 保持边界("允许(C:\data)" 合法)。
/// 非 ASCII 按 UTF-8 解码:汉字等 alphanumeric 视为文件名延续,
/// 中文标点(。、)视为边界。
fn is_path_continuation_after(bytes: &[u8], end: usize) -> bool {
    let byte = bytes[end];
    if byte.is_ascii() {
        is_ascii_continuation(byte) || byte == b'('
    } else {
        std::str::from_utf8(&bytes[end..])
            .ok()
            .and_then(|text| text.chars().next())
            .is_some_and(|character| character.is_alphanumeric())
    }
}

fn is_ascii_continuation(byte: u8) -> bool {
    matches!(
        byte,
        b'a'..=b'z'
            | b'A'..=b'Z'
            | b'0'..=b'9'
            | b'_'
            | b'-'
            | b'.'
            | b'$'
            | b'%'
            | b'~'
            | b'#'
            | b'@'
            | b'!'
            | b'+'
            | b'&'
            | b'{'
            | b'['
            | b'^'
    )
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

    #[test]
    fn quote_path_matching_respects_token_boundaries() {
        // 精确路径 token 匹配通过。
        assert!(quote_contains_path("我允许访问 C:\\data 目录", r"C:\data"));
        assert!(quote_contains_path("允许访问 C:\\data 后继续", r"C:\data"));
        // 中文标点结尾与中文动词紧贴路径不再误拒(review 修复)。
        assert!(quote_contains_path("允许访问 C:\\data。", r"C:\data"));
        assert!(quote_contains_path("允许访问 C:\\资料。", r"C:\资料"));
        assert!(quote_contains_path("授权C:\\data 目录", r"C:\data"));
        assert!(quote_contains_path("允许C:\\资料", r"C:\资料"));
        // 括号包裹路径不再误拒(最终 review 修复)。
        assert!(quote_contains_path("允许访问(C:\\data 目录)", r"C:\data"));
        assert!(quote_contains_path("允许(C:\\data)", r"C:\data"));
        // 但 after 侧括号仍是延续:C:\data(1) 不被 C:\data 误匹配。
        assert!(!quote_contains_path(
            "允许访问 C:\\data(1) 目录",
            r"C:\data"
        ));
        // Windows 合法文件名字符(# @ ! + & { [ ^)作为延续。
        assert!(!quote_contains_path("允许访问 C:\\data#1", r"C:\data"));
        assert!(!quote_contains_path("允许访问 C:\\data@x", r"C:\data"));
        assert!(!quote_contains_path("允许访问 C:\\data[1]", r"C:\data"));
        // 相邻目录名不再误匹配(旧 contains 行为会误匹配)。
        assert!(!quote_contains_path(
            "我允许访问 C:\\data2 目录",
            r"C:\data"
        ));
        assert!(!quote_contains_path(
            "我允许访问 C:\\data-archive",
            r"C:\data"
        ));
        assert!(!quote_contains_path("允许访问 C:\\资料库", r"C:\资料"));
        // 子路径引用仍视为匹配(保留原语义)。
        assert!(quote_contains_path("我允许访问 C:\\data\\sub", r"C:\data"));
        // 大小写不敏感 + 正斜杠归一。
        assert!(quote_contains_path("allow c:/DATA now", r"C:\data"));
        // 中文路径自身可匹配(中文字符是路径的一部分)。
        assert!(quote_contains_path("授权 C:\\资料 目录", r"C:\资料"));
        // 未出现路径时拒绝。
        assert!(!quote_contains_path("允许访问 D:\\other", r"C:\data"));
    }

    #[test]
    fn quote_token_matching_is_case_sensitive_and_bounded() {
        assert!(quote_contains_token("run script.ps1 -f -o out", "-f"));
        assert!(!quote_contains_token("run config-file setup", "-f"));
        assert!(!quote_contains_token("run script -F", "-f"));
        assert!(quote_contains_token(
            "run script.ps1 \"C:\\my path\\x\"",
            r"C:\my path\x"
        ));
        assert!(quote_contains_token("empty token", ""));
    }

    #[test]
    fn quote_negation_is_rejected() {
        // 否定句不得被当作显式授权。
        assert!(quote_contains_negation("不要授权访问 C:\\data"));
        assert!(quote_contains_negation("不允许访问 C:\\data"));
        assert!(quote_contains_negation("拒绝访问 C:\\data"));
        assert!(quote_contains_negation("don't allow access to C:\\data"));
        assert!(quote_contains_negation("I do not allow access to C:\\data"));
        assert!(quote_contains_negation(
            "I give no permission to access C:\\data"
        ));
        assert!(quote_contains_negation("I allow no access to C:\\data"));
        assert!(quote_contains_negation("I dont allow access to C:\\data"));
        assert!(quote_contains_negation("I can't allow access to C:\\data"));
        assert!(quote_contains_negation("I wont allow access to C:\\data"));
        assert!(quote_contains_negation("No! I allow access to C:\\data"));
        assert!(quote_contains_negation(
            "I give no. permission to access C:\\data"
        ));
        assert!(quote_contains_negation("I authorize nothing on C:\\data"));
        assert!(quote_contains_negation("I denied access to C:\\data"));
        assert!(quote_contains_negation(
            "I refused to allow access to C:\\data"
        ));
        assert!(quote_contains_negation(
            "I wouldn't allow access to C:\\data"
        ));
        assert!(quote_contains_negation(
            "I couldn't allow access to C:\\data"
        ));
        assert!(quote_contains_negation(
            "It isn't safe to allow access to C:\\data"
        ));
        assert!(quote_contains_negation("不可以允许访问 C:\\data"));
        assert!(quote_contains_negation("严禁允许访问 C:\\data"));
        assert!(quote_contains_negation("不得允许访问 C:\\data"));
        assert!(quote_contains_negation("我不授权访问 C:\\data"));
        assert!(quote_contains_negation("未经允许访问 C:\\data"));
        assert!(quote_contains_negation("我不批准访问 C:\\data"));
        assert!(quote_contains_negation(
            "without permission to access C:\\data"
        ));
        assert!(quote_contains_negation("别允许访问 C:\\data"));
        assert!(quote_contains_negation("别授权访问 C:\\data"));
        assert!(quote_contains_negation(
            "I withhold permission to access C:\\data"
        ));
        assert!(quote_contains_negation("I authorize nowhere on C:\\data"));
        assert!(quote_contains_negation("I denies access to C:\\data"));
        // 肯定句不含否定词,不应误判。
        assert!(!quote_contains_negation("我允许访问 C:\\data 目录"));
        assert!(!quote_contains_negation("请授权访问 C:\\data"));
        assert!(!quote_contains_negation("I allow access to C:\\data"));
        assert!(!quote_contains_negation(
            "I give permission to access C:\\data"
        ));
        assert!(!quote_contains_negation(
            "I authorize node access on C:\\data"
        ));
    }
}

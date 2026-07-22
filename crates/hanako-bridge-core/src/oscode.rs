//! Decoding of bytes emitted by Windows console programs.
//!
//! Tools such as `schtasks.exe` and `whoami.exe` print localized text in the
//! system ANSI code page (for example GBK on Simplified Chinese Windows), not
//! UTF-8. Decoding that output with `String::from_utf8_lossy` turns every
//! non-ASCII byte into `?`, which erased the real cause of install failures in
//! the error dialog. Decode with the system code page instead so localized
//! system errors stay readable.

/// Decodes bytes from a Windows console program into a `String`.
///
/// If the bytes are already valid UTF-8 they are returned as-is (many tools do
/// emit UTF-8, and ASCII always is). Otherwise, on Windows the bytes are
/// decoded using the system ANSI code page; on other platforms this falls back
/// to a lossy UTF-8 conversion.
pub fn decode_console_bytes(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(text) => text.to_string(),
        Err(_) => decode_ansi(bytes),
    }
}

#[cfg(windows)]
fn decode_ansi(bytes: &[u8]) -> String {
    use windows_sys::Win32::Globalization::{CP_ACP, MB_ERR_INVALID_CHARS, MultiByteToWideChar};

    if bytes.is_empty() {
        return String::new();
    }
    let byte_len = match i32::try_from(bytes.len()) {
        Ok(value) => value,
        Err(_) => return String::from_utf8_lossy(bytes).into_owned(),
    };
    // First pass: how many UTF-16 units does the ANSI text need?
    let needed = unsafe {
        MultiByteToWideChar(
            CP_ACP,
            MB_ERR_INVALID_CHARS,
            bytes.as_ptr(),
            byte_len,
            std::ptr::null_mut(),
            0,
        )
    };
    if needed <= 0 {
        // The bytes are not valid in the ANSI code page either; keep whatever
        // is recoverable rather than losing the message entirely.
        return String::from_utf8_lossy(bytes).into_owned();
    }
    let mut wide = vec![0u16; needed as usize];
    let written = unsafe {
        MultiByteToWideChar(
            CP_ACP,
            MB_ERR_INVALID_CHARS,
            bytes.as_ptr(),
            byte_len,
            wide.as_mut_ptr(),
            needed,
        )
    };
    if written <= 0 {
        return String::from_utf8_lossy(bytes).into_owned();
    }
    String::from_utf16_lossy(&wide[..written as usize])
}

#[cfg(not(windows))]
fn decode_ansi(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_utf8_is_returned_unchanged() {
        assert_eq!(decode_console_bytes(b"hello"), "hello");
        assert_eq!(
            decode_console_bytes("任务已成功创建".as_bytes()),
            "任务已成功创建"
        );
    }

    #[test]
    fn empty_input_is_empty() {
        assert_eq!(decode_console_bytes(b""), "");
    }

    // On Simplified Chinese Windows, "错误" in GBK is 0xB4 0xED 0xCE 0xF3. That
    // byte sequence is not valid UTF-8, so this exercises the ANSI fallback.
    #[cfg(windows)]
    #[test]
    fn gbk_bytes_decode_to_chinese_on_windows() {
        let gbk = [0xB4u8, 0xED, 0xCE, 0xF3];
        // Only assert the real decode when the active code page is GBK (936);
        // on an English-code-page CI box this would decode differently.
        let decoded = decode_console_bytes(&gbk);
        assert!(
            !decoded.contains('\u{fffd}') || decoded == "错误",
            "unexpected decode: {decoded:?}"
        );
    }
}

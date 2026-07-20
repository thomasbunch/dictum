//! Clipboard snapshot / write / restore for the paste-injection backend
//! (clipboard-win 5.4.1). Only ONE `Clipboard` guard may exist at a time across
//! the whole program, so every helper opens, does its work, and drops the guard
//! before returning — never hold one across a call into another helper.

use clipboard_win::{raw, Clipboard, SysResult};

// Cloud Clipboard / Clipboard History privacy-exclusion formats (MS Learn).
const FMT_EXCLUDE: &str = "ExcludeClipboardContentFromMonitorProcessing";
const FMT_HISTORY: &str = "CanIncludeInClipboardHistory";
const FMT_CLOUD: &str = "CanUploadToCloudClipboard";

/// Current CF_UNICODETEXT, or `None` when the clipboard holds non-text
/// (image/files) or nothing. `None` means "restore nothing" after the paste —
/// faithfully restoring non-text is impractical (espanso #2059).
pub fn snapshot_text() -> Option<String> {
    let _clip = Clipboard::new_attempts(10).ok()?;
    let mut buf = Vec::new();
    raw::get_string(&mut buf).ok()?; // errors if CF_UNICODETEXT absent
    Some(String::from_utf8_lossy(&buf).into_owned())
}

/// Write `text` as CF_UNICODETEXT, marked excluded from Clipboard History and
/// Cloud Clipboard sync. Newlines are normalized to CRLF for Win32 paste targets.
pub fn write_excluded(text: &str) -> SysResult<()> {
    let text = normalize_newlines(text);
    let _clip = Clipboard::new_attempts(10)?;
    // set_string empties the clipboard first — it MUST run before the exclusion
    // formats, which are layered on with set_without_clear (no empty).
    raw::set_string(&text)?;
    if let Some(fmt) = raw::register_format(FMT_EXCLUDE) {
        raw::set_without_clear(fmt.get(), &[])?; // empty payload is sufficient
    }
    if let Some(fmt) = raw::register_format(FMT_HISTORY) {
        raw::set_without_clear(fmt.get(), &0u32.to_ne_bytes())?; // DWORD 0 = exclude
    }
    if let Some(fmt) = raw::register_format(FMT_CLOUD) {
        raw::set_without_clear(fmt.get(), &0u32.to_ne_bytes())?; // DWORD 0 = exclude
    }
    Ok(())
}

/// Restore previously-snapshotted text (plain CF_UNICODETEXT, no exclusion
/// formats — it is the user's own prior content).
pub fn restore_text(text: &str) -> SysResult<()> {
    let _clip = Clipboard::new_attempts(10)?;
    raw::set_string(text)?;
    Ok(())
}

/// `\n` -> `\r\n` without doubling an existing `\r\n`.
fn normalize_newlines(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\n', "\r\n")
}

#[cfg(test)]
mod tests {
    use super::normalize_newlines;

    #[test]
    fn crlf_normalization() {
        assert_eq!(normalize_newlines("a\nb"), "a\r\nb");
        assert_eq!(normalize_newlines("a\r\nb"), "a\r\nb"); // no doubling
        assert_eq!(normalize_newlines("a\n\nb"), "a\r\n\r\nb");
        assert_eq!(normalize_newlines("plain"), "plain");
        assert_eq!(normalize_newlines("end\n"), "end\r\n");
    }
}

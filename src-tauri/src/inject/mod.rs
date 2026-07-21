//! Text injection: clipboard-swap + Ctrl+V primary, batched SendInput-unicode
//! fallback. `inject()` runs on the coordinator thread (blocking here is fine).

mod clipboard;
mod integrity;
mod overrides;
mod sendinput;

use crate::types::{Config, InjectBackend, InjectMethod, InjectOutcome, PasteShortcut};
use std::time::Duration;
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

/// Inject `text` into `target_hwnd` (the window focused at hotkey-down). Runs the
/// full fallback chain and reports which terminal state was reached.
pub fn inject(text: &str, target_hwnd: isize, cfg: &Config) -> InjectOutcome {
    // Normalize newlines to CRLF once, for EVERY backend and fallback path — the
    // SendInput-unicode fallback would otherwise emit bare LF into Win32 targets
    // (RDP/Citrix default to it). Idempotent, so downstream writes are safe.
    let normalized = normalize_newlines(text);
    let text = normalized.as_str();

    // (1) Foreground must still be the window we captured at hotkey-down.
    let foreground = unsafe { GetForegroundWindow() };
    if foreground.0 as isize != target_hwnd {
        // Text still reaches the user via the clipboard + held for PasteLast — but
        // only claim the copy if the write actually succeeded (PLAN §1.4).
        return match clipboard::write_excluded(text) {
            Ok(()) => InjectOutcome::FocusChanged,
            Err(_) => InjectOutcome::ClipboardUnavailable,
        };
    }

    // (2) Elevated target: paste/SendInput no-op silently under UIPI — copy only.
    if integrity::foreground_is_elevated() {
        return match clipboard::write_excluded(text) {
            Ok(()) => InjectOutcome::ElevatedClipboardOnly,
            Err(_) => InjectOutcome::ClipboardUnavailable,
        };
    }

    // (3) Don't fire the paste chord while the user's PTT modifiers are still down.
    sendinput::wait_for_modifiers_released();

    // (4) Resolve the per-app backend.
    let target = HWND(target_hwnd as *mut core::ffi::c_void);
    let r = overrides::resolve(overrides::exe_name_for_hwnd(target), &cfg.app_overrides);

    // (5)/(6) run the chosen backend.
    let chars = text.chars().count() as u32;
    let (ok, method) = match r.backend {
        InjectBackend::Clipboard => (inject_clipboard(text, r.paste), InjectMethod::Pasted),
        InjectBackend::SendInputUnicode => {
            (sendinput::send_unicode_string(text, r.chunk_delay_ms), InjectMethod::Typed)
        }
    };

    if ok {
        InjectOutcome::Injected { chars, method }
    } else {
        // (7) Terminal state: leave the text on the clipboard for a manual paste —
        // unless even that write fails, which we must report honestly.
        match clipboard::write_excluded(text) {
            Ok(()) => InjectOutcome::ClipboardManual(text.to_string()),
            Err(_) => InjectOutcome::ClipboardUnavailable,
        }
    }
}

/// Exe file name for the process owning `hwnd` (history per-app column, PLAN §9).
pub fn exe_for_hwnd(hwnd: isize) -> Option<String> {
    overrides::exe_name_for_hwnd(HWND(hwnd as *mut core::ffi::c_void))
}

/// `\n` -> `\r\n` for Win32 paste/SendInput targets, without doubling an existing
/// `\r\n`. Applied once in `inject()` so both backends get CRLF.
fn normalize_newlines(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\n', "\r\n")
}

/// Clipboard-swap backend: snapshot text-only, write transcript + exclusion
/// formats, paste, then restore the snapshot off-thread after 300 ms (past the
/// paste, off the latency-critical path). Non-text prior clipboard => no restore.
fn inject_clipboard(text: &str, paste: PasteShortcut) -> bool {
    let snapshot = clipboard::snapshot_text(); // None => restore nothing
    if clipboard::write_excluded(text).is_err() {
        return false; // clipboard unavailable after retries -> fall to ClipboardManual
    }
    sendinput::send_paste(paste);
    if let Some(prev) = snapshot {
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            let _ = clipboard::restore_text(&prev);
        });
    }
    true
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

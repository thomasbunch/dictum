//! Text injection: clipboard-swap + Ctrl+V primary, batched SendInput-unicode
//! fallback. `inject()` runs on the coordinator thread (blocking here is fine).

mod clipboard;
mod integrity;
mod overrides;
mod sendinput;

use crate::types::{Config, InjectBackend, InjectOutcome, PasteShortcut};
use std::time::Duration;
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

/// Inject `text` into `target_hwnd` (the window focused at hotkey-down). Runs the
/// full fallback chain and reports which terminal state was reached.
pub fn inject(text: &str, target_hwnd: isize, cfg: &Config) -> InjectOutcome {
    // (1) Foreground must still be the window we captured at hotkey-down.
    let foreground = unsafe { GetForegroundWindow() };
    if foreground.0 as isize != target_hwnd {
        // Text still reaches the user: on the clipboard + held for PasteLast.
        let _ = clipboard::write_excluded(text);
        return InjectOutcome::FocusChanged;
    }

    // (2) Elevated target: paste/SendInput no-op silently under UIPI — copy only.
    if integrity::foreground_is_elevated() {
        let _ = clipboard::write_excluded(text);
        return InjectOutcome::ElevatedClipboardOnly;
    }

    // (3) Don't fire the paste chord while the user's PTT modifiers are still down.
    sendinput::wait_for_modifiers_released();

    // (4) Resolve the per-app backend.
    let target = HWND(target_hwnd as *mut core::ffi::c_void);
    let r = overrides::resolve(overrides::exe_name_for_hwnd(target), &cfg.app_overrides);

    // (5)/(6) run the chosen backend.
    let chars = text.chars().count() as u32;
    let ok = match r.backend {
        InjectBackend::Clipboard => inject_clipboard(text, r.paste),
        InjectBackend::SendInputUnicode => sendinput::send_unicode_string(text, r.chunk_delay_ms),
    };

    if ok {
        InjectOutcome::Injected { chars }
    } else {
        // (7) Terminal state: leave the text on the clipboard for a manual paste.
        let _ = clipboard::write_excluded(text);
        InjectOutcome::ClipboardManual(text.to_string())
    }
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

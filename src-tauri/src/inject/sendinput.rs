//! SendInput helpers (windows 0.62.2): unicode text sender (fallback backend),
//! the Ctrl+V / Ctrl+Shift+V paste chord, and the pre-paste modifier-release
//! wait. NEVER per-VK simulation of text — KEYEVENTF_UNICODE only.

use crate::types::PasteShortcut;
use std::time::{Duration, Instant};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS,
    KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, VIRTUAL_KEY, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN,
    VK_SHIFT, VK_V,
};

const CHUNK: usize = 64; // INPUT structs per SendInput call (Win11 Notepad buffering / burst cap)

fn unicode_key(unit: u16, key_up: bool) -> INPUT {
    let flags = if key_up {
        KEYEVENTF_UNICODE | KEYEVENTF_KEYUP
    } else {
        KEYEVENTF_UNICODE
    };
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(0), // must be 0 when KEYEVENTF_UNICODE is set
                wScan: unit,          // UTF-16 code unit
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

/// One down + one up INPUT per UTF-16 code unit (surrogate pairs => 2 units => 4 events).
fn build_events(text: &str) -> Vec<INPUT> {
    text.encode_utf16()
        .flat_map(|u| [unicode_key(u, false), unicode_key(u, true)])
        .collect()
}

/// Type `text` via KEYEVENTF_UNICODE pairs, `CHUNK` events per SendInput call,
/// sleeping `chunk_delay_ms` between chunks. Returns false if any call inserted
/// fewer events than requested (blocked input / UIPI) — caller then falls back.
pub fn send_unicode_string(text: &str, chunk_delay_ms: u64) -> bool {
    let events = build_events(text);
    let mut ok = true;
    for (i, chunk) in events.chunks(CHUNK).enumerate() {
        if i > 0 {
            std::thread::sleep(Duration::from_millis(chunk_delay_ms));
        }
        let sent = unsafe { SendInput(chunk, core::mem::size_of::<INPUT>() as i32) };
        if sent as usize != chunk.len() {
            ok = false;
        }
    }
    ok
}

fn vk_input(vk: VIRTUAL_KEY, key_up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: if key_up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

/// Ctrl+V (or Ctrl+Shift+V) paste chord.
pub fn send_paste(shortcut: PasteShortcut) {
    let shift = matches!(shortcut, PasteShortcut::CtrlShiftV);
    let mut inputs = vec![vk_input(VK_CONTROL, false)];
    if shift {
        inputs.push(vk_input(VK_SHIFT, false));
    }
    inputs.push(vk_input(VK_V, false));
    inputs.push(vk_input(VK_V, true));
    if shift {
        inputs.push(vk_input(VK_SHIFT, true));
    }
    inputs.push(vk_input(VK_CONTROL, true));
    unsafe { SendInput(&inputs, core::mem::size_of::<INPUT>() as i32) };
}

fn key_is_down(vk: i32) -> bool {
    (unsafe { GetAsyncKeyState(vk) } as u16 & 0x8000) != 0
}

/// Block until Ctrl/Shift/Alt/Win are all physically released, or 2 s elapses
/// (then proceed anyway — never hang the coordinator). Stops a still-held PTT
/// modifier from turning our Ctrl+V into Ctrl+Shift+V etc.
pub fn wait_for_modifiers_released() {
    let mods = [VK_CONTROL.0, VK_SHIFT.0, VK_MENU.0, VK_LWIN.0, VK_RWIN.0];
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(2) {
        if !mods.iter().any(|&vk| key_is_down(vk as i32)) {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(test)]
mod tests {
    use super::build_events;

    #[test]
    fn ascii_event_pairs() {
        assert_eq!(build_events("abc").len(), 6); // 2 events per code unit
        assert_eq!(build_events("").len(), 0);
    }

    #[test]
    fn surrogate_pair_emoji() {
        // "😀" is one char but two UTF-16 code units => 4 events.
        let s = "a😀b";
        assert_eq!(s.chars().count(), 3); // Injected{chars} reports this...
        assert_eq!(s.encode_utf16().count(), 4); // ...while SendInput drives utf16 units
        assert_eq!(build_events(s).len(), 8);
    }

    #[test]
    fn chunk_counts() {
        // 40 chars -> 80 events -> ceil(80/64) = 2 chunks.
        assert_eq!(build_events(&"x".repeat(40)).chunks(64).count(), 2);
        // exactly 32 chars -> 64 events -> 1 chunk.
        let full = "y".repeat(32);
        assert_eq!(build_events(&full).len(), 64);
        assert_eq!(build_events(&full).chunks(64).count(), 1);
        // 33 chars -> 66 events -> 2 chunks.
        assert_eq!(build_events(&"z".repeat(33)).chunks(64).count(), 2);
    }
}

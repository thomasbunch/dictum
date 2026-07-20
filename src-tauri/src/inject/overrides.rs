//! Per-app injection backend resolution + exe-name lookup from an HWND.

use crate::types::{AppOverride, InjectBackend, PasteShortcut};
use std::collections::BTreeMap;
use windows::core::PWSTR;
use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;

/// Effective injection settings after merging any per-app override onto defaults.
pub struct Resolved {
    pub backend: InjectBackend,
    pub paste: PasteShortcut,
    pub chunk_delay_ms: u64,
}

/// Defaults: clipboard-paste, Ctrl+V, 5 ms between SendInput chunks. An override
/// keyed by the (lowercased) exe name replaces each field it sets; unset fields
/// keep the default. (RDP/terminal defaults live in `default_app_overrides`.)
pub fn resolve(exe: Option<String>, overrides: &BTreeMap<String, AppOverride>) -> Resolved {
    let mut r = Resolved {
        backend: InjectBackend::Clipboard,
        paste: PasteShortcut::CtrlV,
        chunk_delay_ms: 5,
    };
    if let Some(o) = exe.and_then(|e| overrides.get(&e.to_lowercase())) {
        if let Some(b) = o.backend {
            r.backend = b;
        }
        if let Some(p) = o.paste_shortcut {
            r.paste = p;
        }
        if let Some(d) = o.chunk_delay_ms {
            r.chunk_delay_ms = d;
        }
    }
    r
}

/// Exe file name (e.g. "chrome.exe") for the process owning `hwnd`, or `None`.
pub fn exe_name_for_hwnd(hwnd: HWND) -> Option<String> {
    unsafe {
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        let process: HANDLE = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;

        let mut buf = [0u16; 260]; // MAX_PATH
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            PWSTR(buf.as_mut_ptr()),
            &mut len,
        )
        .is_ok();
        let _ = CloseHandle(process);
        if !ok {
            return None;
        }
        let full = String::from_utf16_lossy(&buf[..len as usize]);
        full.rsplit(['\\', '/']).next().map(|s| s.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::resolve;
    use crate::types::{default_app_overrides, AppOverride, InjectBackend, PasteShortcut};
    use std::collections::BTreeMap;

    #[test]
    fn defaults_when_no_override() {
        let r = resolve(Some("notepad.exe".into()), &BTreeMap::new());
        assert_eq!(r.backend, InjectBackend::Clipboard);
        assert_eq!(r.paste, PasteShortcut::CtrlV);
        assert_eq!(r.chunk_delay_ms, 5);
    }

    #[test]
    fn defaults_when_no_exe() {
        let r = resolve(None, &default_app_overrides());
        assert_eq!(r.backend, InjectBackend::Clipboard);
        assert_eq!(r.paste, PasteShortcut::CtrlV);
    }

    #[test]
    fn rdp_default_forces_sendinput() {
        let r = resolve(Some("mstsc.exe".into()), &default_app_overrides());
        assert_eq!(r.backend, InjectBackend::SendInputUnicode);
        assert_eq!(r.paste, PasteShortcut::CtrlV); // unset -> default
    }

    #[test]
    fn terminal_default_uses_ctrl_shift_v() {
        let r = resolve(Some("windowsterminal.exe".into()), &default_app_overrides());
        assert_eq!(r.backend, InjectBackend::Clipboard); // unset -> default
        assert_eq!(r.paste, PasteShortcut::CtrlShiftV);
    }

    #[test]
    fn lookup_is_case_insensitive() {
        let r = resolve(Some("MSTSC.EXE".into()), &default_app_overrides());
        assert_eq!(r.backend, InjectBackend::SendInputUnicode);
    }

    #[test]
    fn partial_override_keeps_other_defaults() {
        let mut m: BTreeMap<String, AppOverride> = BTreeMap::new();
        m.insert(
            "foo.exe".into(),
            AppOverride { chunk_delay_ms: Some(20), ..Default::default() },
        );
        let r = resolve(Some("foo.exe".into()), &m);
        assert_eq!(r.backend, InjectBackend::Clipboard);
        assert_eq!(r.paste, PasteShortcut::CtrlV);
        assert_eq!(r.chunk_delay_ms, 20);
    }
}

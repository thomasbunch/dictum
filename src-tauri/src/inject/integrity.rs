//! UIPI pre-check: is the foreground window owned by a higher-integrity
//! (elevated) process? SendInput/paste into such a window no-ops silently, so we
//! detect it and fall back to clipboard-only. Fail-safe: any query failure is
//! treated as elevated — never claim we injected when we could not.

use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Security::{
    GetSidSubAuthority, GetSidSubAuthorityCount, GetTokenInformation, TokenIntegrityLevel,
    TOKEN_MANDATORY_LABEL, TOKEN_QUERY,
};
use windows::Win32::System::Threading::{
    OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

// winnt.h SECURITY_MANDATORY_HIGH_RID — not a confirmed windows-rs symbol; hardcode.
const SECURITY_MANDATORY_HIGH_RID: u32 = 0x3000;

/// True if the current foreground window belongs to an elevated (>= HIGH IL)
/// process. Called after the caller has already confirmed foreground == target.
pub fn foreground_is_elevated() -> bool {
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.0.is_null() {
            return false;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));

        let process = match OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
            Ok(h) => h,
            Err(_) => return true, // query denied => conservatively assume elevated
        };

        let mut token = HANDLE::default();
        if OpenProcessToken(process, TOKEN_QUERY, &mut token).is_err() {
            let _ = CloseHandle(process);
            return true;
        }

        // TOKEN_MANDATORY_LABEL + its inline SID. u64 array => 8-byte aligned for
        // the SID pointer; 64 bytes is ample for an integrity-level label.
        let mut buf = [0u64; 8];
        let mut ret_len = 0u32;
        let queried = GetTokenInformation(
            token,
            TokenIntegrityLevel,
            Some(buf.as_mut_ptr() as *mut core::ffi::c_void),
            core::mem::size_of_val(&buf) as u32,
            &mut ret_len,
        )
        .is_ok();

        let is_high = if queried {
            let label = &*(buf.as_ptr() as *const TOKEN_MANDATORY_LABEL);
            let sid = label.Label.Sid;
            let count = *GetSidSubAuthorityCount(sid);
            let rid = *GetSidSubAuthority(sid, (count - 1) as u32);
            rid >= SECURITY_MANDATORY_HIGH_RID
        } else {
            true // couldn't read the IL => fail safe
        };

        let _ = CloseHandle(token);
        let _ = CloseHandle(process);
        is_high
    }
}

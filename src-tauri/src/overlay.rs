//! Recording HUD overlay window control (DESIGN.md §2). Rust shows/hides and
//! positions the window; the webview only paints. Positioned bottom-center of
//! the monitor under the cursor, click-through, never focusable.

use tauri::{AppHandle, Manager, PhysicalPosition};

/// DESIGN §5.6: HUD bottom edge sits 24px above the work-area bottom (above the
/// taskbar). We query the real per-monitor work area (Win32 GetMonitorInfo), so
/// this is the true gap — no taskbar guessing.
const GAP_LOGICAL: f64 = 24.0;

/// Fallback gap when the work-area query fails: the 24px design gap plus a
/// 48px taskbar allowance (full-monitor rect includes the taskbar).
const GAP_LOGICAL_FALLBACK: f64 = 24.0 + 48.0;

/// Call once at startup: make the overlay click-through (the default state).
pub fn setup(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("overlay") {
        let _ = w.set_ignore_cursor_events(true);
    }
}

/// DESIGN §5.6: the HUD is click-through EXCEPT during confirm_discard and
/// error. Window calls off the coordinator thread proved unreliable — hop to
/// the main thread like show/hide.
pub fn set_click_through(app: &AppHandle, click_through: bool) {
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        if let Some(w) = handle.get_webview_window("overlay") {
            if let Err(e) = w.set_ignore_cursor_events(click_through) {
                eprintln!("overlay: set_ignore_cursor_events failed: {e}");
            }
        }
    });
}

/// Position bottom-center on the cursor's monitor and show. No animation here —
/// the webview fades opacity (§2.4).
///
/// Runs on the main thread: the coordinator calls this from its own thread and
/// window show/position/monitor queries proved unreliable off-main (observed:
/// window stayed hidden at its default position). run_on_main_thread is the
/// documented-safe path; failures are logged, never swallowed (PLAN §1.4).
pub fn position_and_show(app: &AppHandle) {
    let handle = app.clone();
    let r = app.run_on_main_thread(move || {
        let Some(w) = handle.get_webview_window("overlay") else { return };
        match (w.cursor_position(), w.outer_size(), w.scale_factor()) {
            (Ok(cursor), Ok(win), Ok(cur_scale)) => match w.monitor_from_point(cursor.x, cursor.y) {
                Ok(Some(mon)) => {
                    let scale = mon.scale_factor();
                    // outer_size() is physical px on the monitor the window is
                    // currently on. After set_position lands it on the target
                    // monitor, WM_DPICHANGED rescales it — so place using the
                    // size it WILL have there, not the stale one (mixed-DPI).
                    let win_w = rescale(win.width, cur_scale, scale);
                    let win_h = rescale(win.height, cur_scale, scale);
                    // Prefer the real work area (excludes taskbar, honors auto-hide
                    // and side-docked taskbars); fall back to full monitor + taskbar
                    // allowance if the Win32 query fails.
                    let (rx, ry, rw, rh, gap_logical) = match work_area(cursor.x as i32, cursor.y as i32) {
                        Some((l, t, r, b)) => {
                            (l, t, (r - l) as u32, (b - t) as u32, GAP_LOGICAL)
                        }
                        None => {
                            let (mp, ms) = (mon.position(), mon.size());
                            (mp.x, mp.y, ms.width, ms.height, GAP_LOGICAL_FALLBACK)
                        }
                    };
                    let (x, y) =
                        bottom_center(rx, ry, rw, rh, win_w, win_h, gap_px(gap_logical, scale));
                    if let Err(e) = w.set_position(PhysicalPosition::new(x, y)) {
                        eprintln!("overlay: set_position failed: {e}");
                    }
                }
                other => eprintln!("overlay: monitor_from_point failed: {other:?}"),
            },
            (c, s, f) => eprintln!(
                "overlay: cursor/size/scale query failed: {:?} {:?} {:?}",
                c.err(),
                s.err(),
                f.err()
            ),
        }
        if let Err(e) = w.show() {
            eprintln!("overlay: show failed: {e}");
        }
    });
    if let Err(e) = r {
        eprintln!("overlay: run_on_main_thread failed: {e}");
    }
}

pub fn hide(app: &AppHandle) {
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        if let Some(w) = handle.get_webview_window("overlay") {
            if let Err(e) = w.hide() {
                eprintln!("overlay: hide failed: {e}");
            }
        }
    });
}

fn gap_px(logical: f64, scale: f64) -> i32 {
    (logical * scale).round() as i32
}

/// Rescale a physical size from the window's current monitor scale to the
/// target monitor's — the size the 320x32-logical strip will have after
/// WM_DPICHANGED. Extracted for testing.
fn rescale(px: u32, from_scale: f64, to_scale: f64) -> u32 {
    (px as f64 / from_scale * to_scale).round() as u32
}

/// Real per-monitor work-area rect (physical px) for the monitor under `(x, y)`,
/// via Win32 GetMonitorInfo. Excludes the taskbar and honors auto-hide / side-
/// docked / non-default-height taskbars. `None` on any query failure.
fn work_area(x: i32, y: i32) -> Option<(i32, i32, i32, i32)> {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST,
    };
    unsafe {
        let hmon = MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if GetMonitorInfoW(hmon, &mut mi).as_bool() {
            let r = mi.rcWork;
            Some((r.left, r.top, r.right, r.bottom))
        } else {
            None
        }
    }
}

/// Pure placement math (physical pixels). Extracted for testing.
fn bottom_center(mon_x: i32, mon_y: i32, mon_w: u32, mon_h: u32, win_w: u32, win_h: u32, gap: i32) -> (i32, i32) {
    let x = mon_x + (mon_w as i32 - win_w as i32) / 2;
    let y = mon_y + mon_h as i32 - win_h as i32 - gap;
    (x, y)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn centers_and_lifts_off_bottom() {
        // 1920x1080 primary, 400x52 HUD, 24px gap.
        let (x, y) = bottom_center(0, 0, 1920, 1080, 400, 52, 24);
        assert_eq!(x, (1920 - 400) / 2);
        assert_eq!(y, 1080 - 52 - 24);
    }

    #[test]
    fn honors_monitor_origin_on_second_display() {
        // second monitor to the right at x=1920.
        let (x, y) = bottom_center(1920, 0, 1280, 720, 400, 52, 24);
        assert_eq!(x, 1920 + (1280 - 400) / 2);
        assert_eq!(y, 720 - 52 - 24);
    }

    #[test]
    fn window_size_rescales_to_target_monitor() {
        // Mixed DPI: HUD last shown on a 100% monitor (400x52 physical),
        // target monitor at 150% -> place as 600x78.
        assert_eq!(rescale(400, 1.0, 1.5), 600);
        assert_eq!(rescale(52, 1.0, 1.5), 78);
        // And back down.
        assert_eq!(rescale(600, 1.5, 1.0), 400);
        assert_eq!(rescale(78, 1.5, 1.0), 52);
        // Uniform DPI: unchanged.
        assert_eq!(rescale(400, 1.25, 1.25), 400);
    }

    #[test]
    fn gap_scales_with_dpi() {
        // 24px design gap above the work-area bottom (§5.6), DPI-scaled.
        assert_eq!(gap_px(GAP_LOGICAL, 1.0), 24);
        assert_eq!(gap_px(GAP_LOGICAL, 1.5), 36);
        // Fallback (full monitor + taskbar allowance).
        assert_eq!(gap_px(GAP_LOGICAL_FALLBACK, 1.0), 72);
    }
}

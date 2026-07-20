//! Recording HUD overlay window control (DESIGN.md §2). Rust shows/hides and
//! positions the window; the webview only paints. Positioned bottom-center of
//! the monitor under the cursor, click-through, never focusable.

use tauri::{AppHandle, Manager, PhysicalPosition};

/// Design gap above the work-area bottom (§2.1) plus a taskbar allowance.
/// ponytail: fixed 48px taskbar allowance instead of Monitor::work_area() —
/// work_area()'s Rect shape is version-fragile in tauri 2.11 and position()/
/// size() are rock-solid. Swap to work_area() if a confirmed signature lands.
const GAP_LOGICAL: f64 = 48.0 + 48.0;

/// Call once at startup: make the overlay permanently click-through.
pub fn setup(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("overlay") {
        let _ = w.set_ignore_cursor_events(true);
    }
}

/// Position bottom-center on the cursor's monitor and show. No animation here —
/// the webview fades opacity (§2.4).
pub fn position_and_show(app: &AppHandle) {
    let Some(w) = app.get_webview_window("overlay") else { return };
    if let (Ok(cursor), Ok(win)) = (w.cursor_position(), w.outer_size()) {
        if let Ok(Some(mon)) = w.monitor_from_point(cursor.x, cursor.y) {
            let scale = mon.scale_factor();
            let (mp, ms) = (mon.position(), mon.size());
            let (x, y) = bottom_center(mp.x, mp.y, ms.width, ms.height, win.width, win.height, gap_px(scale));
            let _ = w.set_position(PhysicalPosition::new(x, y));
        }
    }
    let _ = w.show();
}

pub fn hide(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("overlay") {
        let _ = w.hide();
    }
}

fn gap_px(scale: f64) -> i32 {
    (GAP_LOGICAL * scale).round() as i32
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
        // 1920x1080 primary, 320x64 pill, 96px gap.
        let (x, y) = bottom_center(0, 0, 1920, 1080, 320, 64, 96);
        assert_eq!(x, (1920 - 320) / 2);
        assert_eq!(y, 1080 - 64 - 96);
    }

    #[test]
    fn honors_monitor_origin_on_second_display() {
        // second monitor to the right at x=1920.
        let (x, y) = bottom_center(1920, 0, 1280, 720, 320, 64, 96);
        assert_eq!(x, 1920 + (1280 - 320) / 2);
        assert_eq!(y, 720 - 64 - 96);
    }

    #[test]
    fn gap_scales_with_dpi() {
        assert_eq!(gap_px(1.0), 96);
        assert_eq!(gap_px(1.5), 144);
    }
}

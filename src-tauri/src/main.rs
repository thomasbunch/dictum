// windows_subsystem = "windows" hides the console in release; keep it in debug
// for logs. The exe is a thin shim over the lib entry point.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    dictum_lib::run()
}

# NEEDS — shell agent

Cross-module assumptions my files (`main/lib/commands/overlay/tray.rs`) code
against. Everything landed so far (audio, asr, model, config, history, inject)
was read and matched exactly. The two items below are **not landed yet** and are
the real integration surface; the rest are small verify-points.

## 1. coordinator.rs (coord agent) — REQUIRED, not yet present

`lib.rs` builds the real `Effects` impl and spawns the state machine. It expects
coordinator.rs to expose exactly:

```rust
use crate::types::{Config, CoordMsg, HudEvent, InjectOutcome};
use crate::audio::Cue;         // Cue enum is audio-owned (audio::cues), re-exported at crate::audio::Cue
use crate::tray::TrayState;    // TrayState enum is shell-owned (crate::tray::TrayState)

pub trait Effects {
    fn audio_start(&mut self, device: Option<String>);
    fn audio_stop(&mut self);
    fn audio_abort(&mut self);
    fn decode(&mut self, generation: u64, samples: Vec<f32>);
    fn capture_foreground(&self) -> i64;                 // raw HWND, captured at HotkeyDown
    fn inject(&mut self, text: String, target: i64) -> InjectOutcome;
    fn history_append(&mut self, raw: &str, text: &str); // no-ops internally per config
    fn play_cue(&mut self, cue: Cue);
    fn hud(&mut self, event: HudEvent);                  // broadcasts + shows/hides the overlay window
    fn tray_state(&mut self, state: TrayState);
    fn rearm_hotkey(&mut self, chord: &str);             // SystemResumed ONLY (see below)
    fn config_changed(&mut self, cfg: &Config);          // refreshes tray binding + cue enable
}

pub fn run<E: Effects>(rx: std::sync::mpsc::Receiver<CoordMsg>, cfg: Config, fx: E);
```

State-machine expectations the shell relies on:
- Applies replacements itself: `crate::replacements::apply(&raw, &cfg)` between
  `DecodeDone` and `inject` (I intentionally did NOT put this on `Effects`).
- Captures the foreground HWND with `fx.capture_foreground()` on `HotkeyDown`,
  stores it, passes it to `fx.inject(text, hwnd)`; `PasteLast` re-captures.
- On `ConfigChanged(cfg)`: update its `Config` copy and call
  `fx.config_changed(&cfg)`. **Do NOT re-register the hotkey here** — the
  `set_config` command already rebinds synchronously (for rollback/conflict UX).
  `fx.rearm_hotkey` is for `SystemResumed` only.
- `fx.hud(HudEvent::State{Hidden})` is what hides the overlay; any non-Hidden
  State shows it. Emit `Hidden` after the timed HUD states (Injected/Cancelled/
  Error) so the window actually hides.
- toast copy == `HudEvent::State{Error{msg}}` with the verbatim DESIGN §1.2 line.

If coord prefers a different Effects shape, this is a mechanical reconcile — the
method bodies in `lib.rs::RealEffects` are one-liners over the landed subsystems.

## 2. hotkey.rs (coord agent) — REQUIRED, not yet present

`lib.rs` (startup + resume) and `commands.rs` (set_config, try_hotkey) call:

```rust
/// Register `chord` as the PTT global shortcut. Idempotent: unregister the prior
/// PTT first. Wire Pressed->CoordMsg::HotkeyDown, Released->HotkeyUp, and manage
/// Esc->Cancel while a session is active. Err(reason) if the combo is taken.
pub fn register(app: &tauri::AppHandle, chord: &str, coord_tx: std::sync::mpsc::Sender<crate::types::CoordMsg>) -> Result<(), String>;

/// Parse + availability check without disturbing the active binding (PRESS KEYS).
pub fn try_hotkey(app: &tauri::AppHandle, chord: &str) -> Result<(), String>;
```

The global-shortcut plugin is installed bare in `lib.rs`
(`tauri_plugin_global_shortcut::Builder::new().build()`); `register` should use
`app.global_shortcut().on_shortcut(...)` for per-shortcut dispatch.

## 3. bindings.ts (orchestrator) — one addition

`commands.rs` adds `copy_text(text: String) -> Result<(), String>` for the
History COPY button (not in the current bindings.ts). Please add:
```ts
copyText: (text: string) => invoke<void>("copy_text", { text }),
```

## 4. inject.rs (inject agent) — optional, deferred

`HistoryRecord.exe` is stored as `None` in v1 (future per-app modes). To capture
it, expose the existing private helper: `pub use overrides::exe_name_for_hwnd;`
in `inject/mod.rs` (`fn(HWND) -> Option<String>`), then `RealEffects::history_append`
can resolve it from the target HWND. Not v1-critical (exe is unused in v1).

## 5. Deviation from task spec (intentional)

- **Earcon filenames use underscores** (`cue_start.wav`, `cue_stop.wav`,
  `cue_discard.wav`, `cue_error.wav`), NOT the hyphenated names in the task
  brief. The landed consumer `audio/cues.rs` reads `["cue_start.wav", ...]`; the
  committed reader is the authority. Hyphen files were removed.

## 6. Verify-points (compile-time, low risk; build-fix loop will catch)

- `tray.rs` uses `TrayIconBuilder::show_menu_on_left_click(false)` — the current
  tauri 2.11 method name (was `menu_on_left_click`). If the name differs, rename.
- `tray.rs::system_uses_light_theme` uses `windows_registry::Key::get_u32(...)`.
  RESEARCH flagged windows-registry method names as unverified; if wrong, use
  `get_value` + parse, or the `windows::Win32::System::Registry` fallback.
- `overlay.rs` positions with `Monitor::position()`/`size()` + a fixed 48px
  taskbar allowance instead of `Monitor::work_area()` (Rect shape is version-
  fragile in 2.11). Swap to `work_area()` if a confirmed signature lands.
- `icon.ico` entries are PNG-embedded (16/32/48/256). If the NSIS bundler rejects
  PNG entries at small sizes, re-emit small entries as BMP in `gen-icons.mjs`.

## No changes needed

`types.rs` (used as-is), `Cargo.toml` (all deps/features I use are present:
tauri tray-icon, windows-registry 0.6, windows Win32_UI_WindowsAndMessaging +
Win32_Foundation, clipboard-win), `tauri.conf.json`, `capabilities/default.json`
(custom commands don't need capability grants in tauri 2). Tray icons load via
`Image::new` (raw RGBA sidecars) so no `image-png` feature is required.

//! Global-shortcut wiring. Forwards raw Pressed/Released to the coordinator;
//! all tap-vs-hold / toggle logic lives in the coordinator, never here.
//!
//! Released on Windows arrives ~0-50ms late via a GetAsyncKeyState poll inside
//! the plugin (no native key-up message) — the coordinator absorbs that in the
//! trailing-silence budget.

use crate::types::CoordMsg;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use tauri::AppHandle;
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Shortcut, ShortcutState};

// The plugin stores handlers in a shared registry, so a handler must be
// Send + Sync. std mpsc::Sender is Send but not Sync — wrap it.
type Tx = Arc<Mutex<Sender<CoordMsg>>>;

pub struct HotkeyManager {
    app: AppHandle,
    tx: Tx,
    chord: Shortcut,
    esc: Shortcut,
    esc_armed: bool,
}

impl HotkeyManager {
    /// Register the PTT chord and start forwarding Pressed/Released.
    /// Registration failure (another app owns the combo) returns Err so the UI
    /// can surface the conflict copy — never silent.
    pub fn register(
        app: AppHandle,
        chord_str: &str,
        coord_tx: Sender<CoordMsg>,
    ) -> Result<Self, String> {
        let chord = parse(chord_str)?;
        let m = HotkeyManager {
            app,
            tx: Arc::new(Mutex::new(coord_tx)),
            chord: chord.clone(),
            esc: Shortcut::new(None, Code::Escape),
            esc_armed: false,
        };
        m.attach_ptt(&chord)?;
        Ok(m)
    }

    /// Construct WITHOUT binding a shortcut. Used when the configured chord is
    /// unavailable at startup so the app still runs and Settings can `rebind`
    /// to a free chord later. No global shortcut is registered here.
    pub fn unbound(app: AppHandle, chord_str: &str, coord_tx: Sender<CoordMsg>) -> Self {
        let chord = parse(chord_str).unwrap_or_else(|_| Shortcut::new(None, Code::Escape));
        HotkeyManager {
            app,
            tx: Arc::new(Mutex::new(coord_tx)),
            chord,
            esc: Shortcut::new(None, Code::Escape),
            esc_armed: false,
        }
    }

    fn attach_ptt(&self, chord: &Shortcut) -> Result<(), String> {
        let tx = self.tx.clone();
        self.app
            .global_shortcut()
            .on_shortcut(chord.clone(), move |_app, _sc, event| {
                let msg = match event.state() {
                    ShortcutState::Pressed => CoordMsg::HotkeyDown,
                    ShortcutState::Released => CoordMsg::HotkeyUp,
                };
                if let Ok(tx) = tx.lock() {
                    let _ = tx.send(msg);
                }
            })
            .map_err(|e| e.to_string())
    }

    fn attach_esc(&self) -> Result<(), String> {
        let tx = self.tx.clone();
        self.app
            .global_shortcut()
            .on_shortcut(self.esc.clone(), move |_app, _sc, event| {
                // Cancel on press only; ignore the key-up.
                if let ShortcutState::Pressed = event.state() {
                    if let Ok(tx) = tx.lock() {
                        let _ = tx.send(CoordMsg::Cancel);
                    }
                }
            })
            .map_err(|e| e.to_string())
    }

    /// Swap the PTT chord. On failure the old binding is restored.
    pub fn rebind(&mut self, chord_str: &str) -> Result<(), String> {
        let new = parse(chord_str)?;
        let _ = self.app.global_shortcut().unregister(self.chord.clone());
        match self.attach_ptt(&new) {
            Ok(()) => {
                self.chord = new;
                Ok(())
            }
            Err(e) => {
                let _ = self.attach_ptt(&self.chord.clone()); // restore old
                Err(e)
            }
        }
    }

    /// Register/unregister plain Esc → Cancel. Esc is bound only while a
    /// session is active.
    pub fn arm_esc(&mut self, armed: bool) -> Result<(), String> {
        if armed == self.esc_armed {
            return Ok(());
        }
        if armed {
            self.attach_esc()?;
        } else {
            let _ = self.app.global_shortcut().unregister(self.esc.clone());
        }
        self.esc_armed = armed;
        Ok(())
    }

    /// SystemResumed / session-unlock re-arm: the hook can silently die after
    /// sleep (Handy #1620). Tear everything down and re-register.
    pub fn rearm(&self) -> Result<(), String> {
        let _ = self.app.global_shortcut().unregister_all();
        self.attach_ptt(&self.chord)?;
        if self.esc_armed {
            self.attach_esc()?;
        }
        Ok(())
    }
}

fn parse(chord: &str) -> Result<Shortcut, String> {
    chord
        .parse::<Shortcut>()
        .map_err(|e| format!("invalid hotkey '{chord}': {e}"))
}

/// Parse + availability check without disturbing the active binding (Settings
/// "PRESS KEYS"). Ok if the chord is free or already ours; Err with the conflict
/// copy if another app owns it. Probes by a transient register/unregister.
pub fn try_hotkey(app: &AppHandle, chord: &str) -> Result<(), String> {
    let sc = parse(chord)?;
    let gs = app.global_shortcut();
    if gs.is_registered(sc.clone()) {
        return Ok(()); // the live PTT binding — available by definition
    }
    gs.register(sc.clone())
        .map_err(|_| "Hotkey unavailable — another app may already use this combination.".to_string())?;
    let _ = gs.unregister(sc);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::parse;

    #[test]
    fn parses_valid_chord_and_rejects_garbage() {
        // The global-shortcut backend (global-hotkey) requires a non-modifier
        // main key. The default chord includes one; modifier-only combos like
        // "Ctrl+Super" are rejected (bare-modifier PTT needs win-hotkeys —
        // PLAN §121, deferred to v1.1).
        assert!(parse("Ctrl+Alt+Space").is_ok());
        assert!(parse("Ctrl+Super").is_err());
        assert!(parse("not a chord").is_err());
    }
}

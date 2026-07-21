//! The app state machine. Single thread, single mpsc receiver. Every listed
//! edge in CONTRACTS.md "Coordinator state machine" is implemented here;
//! unlisted (state, msg) pairs are ignored with a debug log.
//!
//! Testability: all side effects go through the `Effects` trait (mocked in
//! tests) and the clock is `Effects::now()`. `Coordinator::handle` /
//! `fire_timer` are pure-ish and driven directly by tests; `run` is the thin
//! recv_timeout loop around them.

use crate::types::*;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

// Timing constants (CONTRACTS + DESIGN §5.6 dwell column).
const TAP_MS: u128 = 400; // tap-vs-hold threshold
const CONFIRM_ELAPSED_S: u64 = 30; // recording length that arms Esc double-confirm
const CONFIRM_WINDOW_MS: u64 = 2000; // ConfirmDiscard revert window
const HUD_INJECTED_MS: u64 = 900;
const HUD_CANCELLED_MS: u64 = 500;
const HUD_ERROR_MS: u64 = 2400;
// §7 M2 hide fade is 100ms in the webview; hide the native window a hair later
// so the fade is never clipped.
const HUD_FADE_MS: u64 = 150;
// ponytail: fixed idle-unload delay; make it a config knob only if RAM tuning demands.
const IDLE_UNLOAD_MS: u64 = 30_000;

/// Earcon kinds. The audio `Cue` enum lives in another module; this is the
/// coordinator-facing alias, mapped in the `Effects` impl.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CueKind {
    Start,
    Stop,
    Discard,
    Error,
}

/// Everything the coordinator does to the outside world. Implemented by the
/// shell; mocked in tests.
pub trait Effects {
    fn start_capture(&mut self, device: Option<String>);
    fn stop_capture(&mut self);
    fn abort_capture(&mut self);
    fn play_cue(&mut self, cue: CueKind);
    fn decode(&mut self, generation: u64, samples: Vec<f32>);
    fn ensure_model(&mut self);
    fn unload_model(&mut self);
    fn inject(&mut self, text: String, target_hwnd: isize) -> InjectOutcome;
    fn capture_foreground(&mut self) -> isize;
    fn hud(&mut self, ev: HudEvent);
    fn show_overlay(&mut self);
    fn hide_overlay(&mut self);
    fn set_tray_recording(&mut self, rec: bool);
    /// Drive the tray to its NO MICROPHONE error state (DESIGN §3.1). Cleared by
    /// the next `set_tray_recording` on the following session start.
    fn set_tray_error(&mut self);
    fn toast(&mut self, msg: String);
    /// Exe file name owning `hwnd`, for the history per-app column (PLAN §9).
    fn foreground_exe(&mut self, hwnd: isize) -> Option<String>;
    fn append_history(&mut self, raw: String, text: String, exe: Option<String>, meta: TakeMeta);
    /// Post-ASR transforms: replacements + file tagging. `target_hwnd` is the
    /// window captured at hotkey-down (file-tag root auto-pick reads its title).
    fn apply_replacements(&mut self, raw: &str, target_hwnd: isize) -> String;
    fn set_esc_armed(&mut self, armed: bool);
    /// Model status changed — surface to the main window (SETUP model card,
    /// masthead status line). Default no-op keeps test mocks small.
    fn announce_model_status(&mut self, _st: &ModelStatus) {}
    /// Config switched the active model SKU — the ASR worker drops the old
    /// recognizer and future loads use the new id. Default no-op for mocks.
    fn set_model(&mut self, _id: String) {}
    /// A transcription is now held for PasteLast — the tray menu item enables.
    fn set_paste_available(&mut self, _on: bool) {}
    fn now(&mut self) -> Instant;
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum State {
    Idle,
    Recording { toggled: bool, started_at: Instant },
    AwaitingTail,
    // Injecting is synchronous (inject() returns immediately), so it is not a
    // resident wait-state — it happens inline in the DecodeDone handler.
    Decoding,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Timer {
    HideHud,
    /// Second phase of HideHud: the webview got Hidden and is fading (§2.4);
    /// now actually hide the native window.
    HideWindow,
    RevertConfirm,
    UnloadIdle,
}

pub struct Coordinator {
    cfg: Config,
    state: State,
    /// Bumped on every session start and on every cancel — stale DecodeDone
    /// (mismatched generation) is dropped, never dropped by timing.
    gen: u64,
    model_status: ModelStatus,

    // Per-session accumulation.
    texts: Vec<String>,     // decoded segment/tail texts in arrival (=dispatch) order
    outstanding: usize,     // decode requests dispatched but not yet answered
    decode_queue: Vec<Vec<f32>>, // segments awaiting model-ready
    tail_sent: bool,        // release/tail reached — finalize once outstanding drains
    started: bool,          // CaptureStarted seen (gates start cue + LISTENING)
    confirm_pending: bool,  // ConfirmDiscard shown, awaiting 2nd Esc
    target_hwnd: isize,     // foreground captured at hotkey-down
    level_amps: Vec<f32>,   // per-bar amplitudes for the take envelope (37.5ms each)
    take_clipped: bool,     // any bar hit >= -1 dBFS this session

    last_text: Option<String>, // for PasteLast
    timer: Option<(Instant, Timer)>,
}

impl Coordinator {
    pub fn new(cfg: Config) -> Self {
        Self {
            cfg,
            state: State::Idle,
            gen: 0,
            model_status: ModelStatus::Ready, // assume shell sends real status before first use
            texts: Vec::new(),
            outstanding: 0,
            decode_queue: Vec::new(),
            tail_sent: false,
            started: false,
            confirm_pending: false,
            target_hwnd: 0,
            level_amps: Vec::new(),
            take_clipped: false,
            last_text: None,
            timer: None,
        }
    }

    fn model_ready(&self) -> bool {
        matches!(self.model_status, ModelStatus::Ready)
    }

    fn loading_pct(&self) -> u8 {
        match self.model_status {
            ModelStatus::Loading { pct } => pct,
            _ => 0,
        }
    }

    fn cue(&mut self, fx: &mut dyn Effects, k: CueKind) {
        if self.cfg.audio_cues {
            fx.play_cue(k);
        }
    }

    fn set_state(&mut self, fx: &mut dyn Effects, s: HudState) {
        fx.hud(HudEvent::State { s });
    }

    fn reset_session(&mut self) {
        self.texts.clear();
        self.outstanding = 0;
        self.decode_queue.clear();
        self.tail_sent = false;
        self.started = false;
        self.confirm_pending = false;
        self.level_amps.clear();
        self.take_clipped = false;
    }

    /// Take metadata for the history record (DESIGN §5.2 expanded row).
    fn take_meta(&self, method: InjectMethod) -> TakeMeta {
        TakeMeta {
            dur_ms: (self.level_amps.len() as f64 * crate::types::BAR_SAMPLES as f64 / 16.0) as i64,
            clipped: self.take_clipped,
            envelope: downsample_envelope(&self.level_amps, 64),
            method: Some(method),
        }
    }

    /// Dispatch a decode now, or queue it if the model isn't ready yet.
    fn dispatch_or_queue(&mut self, fx: &mut dyn Effects, samples: Vec<f32>) {
        if samples.is_empty() {
            return;
        }
        if self.model_ready() {
            fx.decode(self.gen, samples);
            self.outstanding += 1;
        } else {
            self.decode_queue.push(samples);
        }
    }

    // --- Session lifecycle ---------------------------------------------------

    fn start_recording(&mut self, fx: &mut dyn Effects, toggled: bool) {
        self.gen = self.gen.wrapping_add(1);
        self.reset_session();
        self.timer = None; // cancel any pending HUD-hide / idle-unload
        self.target_hwnd = fx.capture_foreground();
        let now = fx.now();
        self.state = State::Recording { toggled, started_at: now };
        fx.show_overlay();
        fx.set_tray_recording(true);
        fx.set_esc_armed(true);

        if self.cfg.unload_on_idle && !self.model_ready() {
            fx.ensure_model();
        }
        fx.start_capture(self.cfg.input_device.clone());

        // Status before first frames: loading if model not ready, else the
        // window is shown but LISTENING waits for CaptureStarted.
        if !self.model_ready() {
            let pct = self.loading_pct();
            self.set_state(fx, HudState::LoadingModel { pct });
        }
    }

    /// Reset machine to Idle and drop session ownership of tray/esc. Does NOT
    /// touch the overlay or timer — the caller sets the terminal HUD + hide timer.
    fn end_session(&mut self, fx: &mut dyn Effects) {
        self.state = State::Idle;
        self.confirm_pending = false;
        fx.set_tray_recording(false);
        fx.set_esc_armed(false);
    }

    fn cancel(&mut self, fx: &mut dyn Effects, abort: bool) {
        if abort {
            fx.abort_capture();
        }
        self.gen = self.gen.wrapping_add(1); // discard any in-flight decode
        self.cue(fx, CueKind::Discard);
        self.set_state(fx, HudState::Cancelled);
        self.end_session(fx);
        self.timer = Some((fx.now() + Duration::from_millis(HUD_CANCELLED_MS), Timer::HideHud));
    }

    /// True while real captured speech is still in flight or decoded — a
    /// mid-hold segment, the death-path tail, or an already-decoded text.
    fn has_pending_audio(&self) -> bool {
        self.outstanding > 0 || !self.decode_queue.is_empty() || !self.texts.is_empty()
    }

    /// Terminal failure: bump generation (drop any in-flight decodes), error cue,
    /// DESIGN §6 wire-voice copy on the HUD, end the session, 2.4 s hide timer.
    /// `tray_error` additionally drives the tray mic-error glyph.
    fn fail(&mut self, fx: &mut dyn Effects, label: &str, detail: &str, tray_error: bool) {
        self.gen = self.gen.wrapping_add(1);
        self.cue(fx, CueKind::Error);
        self.set_state(fx, HudState::Error { label: label.into(), detail: detail.into() });
        self.end_session(fx);
        if tray_error {
            fx.set_tray_error();
        }
        self.timer = Some((fx.now() + Duration::from_millis(HUD_ERROR_MS), Timer::HideHud));
    }

    fn to_awaiting_tail(&mut self, fx: &mut dyn Effects) {
        fx.stop_capture();
        self.cue(fx, CueKind::Stop); // stop cue plays BEFORE decode
        self.state = State::AwaitingTail;
    }

    fn enter_decoding(&mut self, fx: &mut dyn Effects) {
        self.tail_sent = true;
        self.state = State::Decoding;
        if self.model_ready() {
            self.set_state(fx, HudState::Transcribing);
        } else {
            let pct = self.loading_pct();
            self.set_state(fx, HudState::LoadingModel { pct });
        }
        self.try_finalize(fx);
    }

    /// Finalize when the tail is in and all decodes have drained.
    fn try_finalize(&mut self, fx: &mut dyn Effects) {
        if self.state != State::Decoding
            || !self.tail_sent
            || self.outstanding != 0
            || !self.decode_queue.is_empty()
        {
            return;
        }
        let raw = self
            .texts
            .iter()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        let text = fx.apply_replacements(&raw, self.target_hwnd);

        if text.trim().is_empty() {
            // Nothing was said (or all filtered) — treat as a discard, no cue.
            self.set_state(fx, HudState::Cancelled);
            self.end_session(fx);
            self.timer =
                Some((fx.now() + Duration::from_millis(HUD_CANCELLED_MS), Timer::HideHud));
            return;
        }
        let outcome = fx.inject(text.clone(), self.target_hwnd);
        self.finish_inject(fx, raw, text, outcome, true);
    }

    /// Non-success injection outcome: hold text for PasteLast, error cue, wire-
    /// voice copy on the HUD, end session, error dwell.
    fn inject_error(&mut self, fx: &mut dyn Effects, text: String, label: &str, detail: &str) {
        self.last_text = Some(text);
        self.cue(fx, CueKind::Error);
        self.set_state(fx, HudState::Error { label: label.into(), detail: detail.into() });
        self.end_session(fx);
        self.timer = Some((fx.now() + Duration::from_millis(HUD_ERROR_MS), Timer::HideHud));
    }

    /// Shared injection-outcome handling (finalize + PasteLast).
    /// `record` gates history append (false for re-injects via PasteLast).
    fn finish_inject(
        &mut self,
        fx: &mut dyn Effects,
        raw: String,
        text: String,
        outcome: InjectOutcome,
        record: bool,
    ) {
        // Every arm ends with a held transcription — the tray Paste item enables.
        fx.set_paste_available(true);
        match outcome {
            InjectOutcome::Injected { chars, method } => {
                self.last_text = Some(text.clone());
                if record {
                    let exe = fx.foreground_exe(self.target_hwnd);
                    let meta = self.take_meta(method);
                    fx.append_history(raw, text, exe, meta);
                }
                self.set_state(fx, HudState::Injected { chars });
                self.end_session(fx);
                self.timer =
                    Some((fx.now() + Duration::from_millis(HUD_INJECTED_MS), Timer::HideHud));
            }
            InjectOutcome::ElevatedClipboardOnly => {
                self.inject_error(fx, text, "PROTECTED WINDOW", "SENT TO CLIPBOARD — PASTE IT");
            }
            InjectOutcome::FocusChanged => {
                // Hold the text; PasteLast re-injects into the now-focused window.
                // inject() already put it on the clipboard — surface that on the
                // HUD (no-silent-failure): toast has no UI in v1.
                self.inject_error(fx, text, "WINDOW CHANGED", "SENT TO CLIPBOARD — PASTE IT");
            }
            InjectOutcome::ClipboardManual(t) => {
                self.inject_error(fx, t, "COULD NOT PRINT", "SENT TO CLIPBOARD — PASTE IT");
            }
            InjectOutcome::ClipboardUnavailable => {
                // The clipboard write genuinely failed — do NOT claim a copy that
                // didn't happen (PLAN §1.4). Text survives only in last_text; the
                // remedy is PasteLast (which retries the write).
                fx.toast("Clipboard busy — could not copy. Use Paste last to retry.".into());
                self.inject_error(fx, text, "CLIPBOARD BUSY", "PASTE LAST TO RETRY");
            }
        }
    }

    fn arm_idle_unload(&mut self, fx: &mut dyn Effects) {
        if self.cfg.unload_on_idle {
            self.timer =
                Some((fx.now() + Duration::from_millis(IDLE_UNLOAD_MS), Timer::UnloadIdle));
        }
    }

    // --- Message handling ----------------------------------------------------

    pub fn handle(&mut self, msg: CoordMsg, fx: &mut dyn Effects) {
        use CoordMsg::*;
        // State is Copy — match by value so field bindings are owned (bool/Instant),
        // not references, and no borrow of self is held into the arms.
        match (self.state, msg) {
            // ---- Start / stop -------------------------------------------------
            (State::Idle, HotkeyDown) => {
                // `use CoordMsg::*` above shadows the type `ModelStatus` with the
                // CoordMsg variant, so qualify the enum type here.
                match self.model_status.clone() {
                    crate::types::ModelStatus::Missing => {
                        self.cue(fx, CueKind::Error);
                        fx.show_overlay();
                        self.set_state(fx, HudState::Error {
                            label: "NO MODEL".into(),
                            detail: "OPEN DICTUM TO DOWNLOAD".into(),
                        });
                        self.timer =
                            Some((fx.now() + Duration::from_millis(HUD_ERROR_MS), Timer::HideHud));
                    }
                    crate::types::ModelStatus::Error(m) => {
                        self.cue(fx, CueKind::Error);
                        fx.show_overlay();
                        self.set_state(fx, HudState::Error {
                            label: "MODEL ERROR".into(),
                            detail: m,
                        });
                        self.timer =
                            Some((fx.now() + Duration::from_millis(HUD_ERROR_MS), Timer::HideHud));
                    }
                    // Ready, Loading, or Unloaded -> capture anyway (no lost words;
                    // start_recording warms an unloaded model).
                    _ => self.start_recording(fx, false),
                }
            }
            // Tray start (explicit toggle command). Not in the edge table but
            // required for the tray "Start dictation" to work.
            (State::Idle, ToggleDictation) => self.start_recording(fx, true),

            (State::Recording { toggled, started_at }, HotkeyUp) => {
                if toggled {
                    self.dbg("HotkeyUp while toggled");
                } else {
                    let held = fx.now().duration_since(started_at).as_millis();
                    let mode_latches =
                        matches!(self.cfg.hotkey_mode, HotkeyMode::Toggle | HotkeyMode::Both);
                    if held < TAP_MS && mode_latches {
                        // Tap: latch toggle on, keep recording.
                        self.state = State::Recording { toggled: true, started_at };
                    } else {
                        self.to_awaiting_tail(fx);
                    }
                }
            }
            // A second key-press ends a latched (toggled) recording.
            (State::Recording { toggled: true, .. }, HotkeyDown) => self.to_awaiting_tail(fx),
            // Tray "Stop dictation" ends any recording.
            (State::Recording { .. }, ToggleDictation) => self.to_awaiting_tail(fx),

            // ---- Cancel -------------------------------------------------------
            (State::Recording { started_at, .. }, Cancel) => {
                let elapsed = fx.now().duration_since(started_at).as_secs();
                if elapsed > CONFIRM_ELAPSED_S && !self.confirm_pending {
                    self.confirm_pending = true;
                    self.set_state(fx, HudState::ConfirmDiscard);
                    self.timer = Some((
                        fx.now() + Duration::from_millis(CONFIRM_WINDOW_MS),
                        Timer::RevertConfirm,
                    ));
                } else {
                    self.cancel(fx, true);
                }
            }
            (State::AwaitingTail, Cancel) | (State::Decoding, Cancel) => self.cancel(fx, false),

            // ---- Capture pipeline --------------------------------------------
            (State::Recording { .. }, CaptureStarted) => {
                if !self.started {
                    self.started = true;
                    self.cue(fx, CueKind::Start); // start cue ONLY after real frames
                    if self.model_ready() {
                        self.set_state(fx, HudState::Listening);
                    }
                }
            }
            (State::Recording { .. }, Levels(bars)) => {
                for b in &bars {
                    self.level_amps.push(b.amp);
                    self.take_clipped |= b.clip;
                }
                fx.hud(HudEvent::Levels { bars });
            }
            (State::Recording { .. }, SegmentClosed(samples)) => {
                self.dispatch_or_queue(fx, samples);
            }
            (State::AwaitingTail, TailSegment(samples)) => {
                self.dispatch_or_queue(fx, samples);
                self.enter_decoding(fx);
            }
            // Death-path tail: on mid-recording device death the worker flushes the
            // tail (finalize) BEFORE sending CaptureDead, while we're still Recording
            // (no HotkeyUp occurred). Accumulate it; the CaptureDead that immediately
            // follows decodes it (PLAN §4.5 — unplug must not lose audio silently).
            (State::Recording { .. }, TailSegment(samples)) => {
                self.dispatch_or_queue(fx, samples);
            }
            // Late segment that closed just before release lands while awaiting
            // the tail — keep accumulating it.
            (State::AwaitingTail, SegmentClosed(samples)) => {
                self.dispatch_or_queue(fx, samples);
            }
            (State::Recording { .. }, CaptureDead(_)) | (State::AwaitingTail, CaptureDead(_)) => {
                if self.has_pending_audio() {
                    // Real captured speech is buffered (incl. the death-path tail) —
                    // decode & inject it, with the PLAN §4.5 error earcon.
                    self.cue(fx, CueKind::Error);
                    self.enter_decoding(fx);
                } else {
                    // The mic never delivered audio (unplugged before speech, no
                    // device, privacy toggle off) — surface NO MICROPHONE verbatim
                    // (DESIGN §5.6) + tray error, not a misleading KILLED.
                    self.fail(fx, "NO MICROPHONE", "PLUG IN OR PICK ANOTHER INPUT", true);
                }
            }
            (_, CaptureDead(_)) => {
                self.cue(fx, CueKind::Error);
            }

            // ---- ASR results --------------------------------------------------
            (_, DecodeDone { generation, text }) => {
                if generation != self.gen {
                    self.dbg("stale DecodeDone dropped");
                } else {
                    self.outstanding = self.outstanding.saturating_sub(1);
                    self.texts.push(text);
                    self.try_finalize(fx);
                }
            }
            (_, DecodeFailed { generation, error }) => {
                if generation == self.gen {
                    // A failed segment must not silently vanish, nor let the other
                    // segments inject as a partial "success" (PLAN §1.4). Abandon the
                    // utterance and surface the error. fail() bumps the generation,
                    // dropping any sibling in-flight decodes.
                    eprintln!("coordinator: decode failed: {error}");
                    self.fail(fx, "DECODE FAILED", "NOTHING PRINTED — TRY AGAIN", false);
                }
            }

            // ---- Model / config / system -------------------------------------
            (_, ModelStatus(st)) => self.on_model_status(fx, st),
            (_, ConfigChanged(c)) => {
                if c.model_id != self.cfg.model_id {
                    // Switch SKUs: drop the old recognizer, then warm the new
                    // one unless the user runs unload-on-idle (it would load
                    // lazily on the next take either way; ensure() surfaces
                    // Missing if the files aren't on disk yet).
                    fx.set_model(c.model_id.clone());
                    if !c.unload_on_idle {
                        fx.ensure_model();
                    }
                }
                self.cfg = c;
            }
            (_, SystemResumed) => {
                // Hotkey re-arm is the hotkey module's job (it holds the handle);
                // nothing for the state machine to do here.
                self.dbg("SystemResumed");
            }

            // ---- Paste-last ---------------------------------------------------
            (State::Idle, PasteLast) => {
                if let Some(text) = self.last_text.clone() {
                    let hwnd = fx.capture_foreground();
                    let outcome = fx.inject(text.clone(), hwnd);
                    fx.show_overlay();
                    self.finish_inject(fx, text.clone(), text, outcome, false);
                }
            }

            (_, Shutdown) => self.dbg("Shutdown (handled by run loop)"),

            (s, m) => eprintln!("coordinator: ignoring {m:?} in {s:?}"),
        }
    }

    fn on_model_status(&mut self, fx: &mut dyn Effects, st: crate::types::ModelStatus) {
        let became_ready = !self.model_ready() && st == crate::types::ModelStatus::Ready;
        self.model_status = st;
        fx.announce_model_status(&self.model_status);
        if became_ready {
            // Flush anything queued while the model was loading.
            let queued = std::mem::take(&mut self.decode_queue);
            for s in queued {
                fx.decode(self.gen, s);
                self.outstanding += 1;
            }
            match self.state {
                State::Recording { .. } => {
                    if self.started {
                        self.set_state(fx, HudState::Listening);
                    }
                }
                State::Decoding => {
                    self.set_state(fx, HudState::Transcribing);
                    self.try_finalize(fx);
                }
                _ => {}
            }
        } else if matches!(self.state, State::Recording { .. }) {
            // pct is Copy, so this reads a copy without moving/borrowing self.
            if let ModelStatus::Loading { pct } = self.model_status {
                self.set_state(fx, HudState::LoadingModel { pct });
            }
        }
    }

    // --- Timers --------------------------------------------------------------

    fn fire_timer(&mut self, fx: &mut dyn Effects) {
        let Some((_, t)) = self.timer.take() else { return };
        match t {
            Timer::HideHud => {
                // Broadcast Hidden so the webview plays the 160ms hide fade
                // (§2.4) and knows to fade back in on the next session.
                self.set_state(fx, HudState::Hidden);
                self.timer =
                    Some((fx.now() + Duration::from_millis(HUD_FADE_MS), Timer::HideWindow));
            }
            Timer::HideWindow => {
                fx.hide_overlay();
                self.arm_idle_unload(fx);
            }
            Timer::RevertConfirm => {
                self.confirm_pending = false;
                // Recording continued underneath; restore the running HUD.
                if matches!(self.state, State::Recording { .. }) && self.model_ready() {
                    self.set_state(fx, HudState::Listening);
                }
            }
            Timer::UnloadIdle => {
                if self.cfg.unload_on_idle {
                    fx.unload_model();
                }
            }
        }
    }

    fn dbg(&self, what: &str) {
        eprintln!("coordinator: {what} in {:?}", self.state);
    }
}

/// Peak-preserving downsample of the per-bar amplitude series to ≤ `max` points
/// (expanded-row trace, DESIGN §5.2). Peaks matter more than means for a level
/// trace — a clipped syllable must survive the reduction.
fn downsample_envelope(amps: &[f32], max: usize) -> Vec<f32> {
    if amps.len() <= max {
        return amps.to_vec();
    }
    (0..max)
        .map(|i| {
            let lo = i * amps.len() / max;
            let hi = ((i + 1) * amps.len() / max).max(lo + 1);
            amps[lo..hi].iter().cloned().fold(0.0f32, f32::max)
        })
        .collect()
}

impl Coordinator {

    // --- Run loop ------------------------------------------------------------

    pub fn run(rx: Receiver<CoordMsg>, fx: &mut dyn Effects, cfg: Config) {
        let mut coord = Coordinator::new(cfg);
        loop {
            let recv = match coord.timer {
                Some((deadline, _)) => {
                    let now = Instant::now();
                    if deadline <= now {
                        coord.fire_timer(fx);
                        continue;
                    }
                    rx.recv_timeout(deadline - now)
                }
                None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
            };
            match recv {
                Ok(CoordMsg::Shutdown) => break,
                Ok(msg) => coord.handle(msg, fx),
                Err(RecvTimeoutError::Timeout) => coord.fire_timer(fx),
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, PartialEq)]
    enum Call {
        StartCapture(Option<String>),
        StopCapture,
        AbortCapture,
        Cue(CueKind),
        Decode { gen: u64, len: usize },
        EnsureModel,
        UnloadModel,
        Inject(String),
        CaptureForeground,
        Hud(String),
        ShowOverlay,
        HideOverlay,
        Tray(bool),
        TrayError,
        Toast(String),
        History { raw: String, text: String, exe: Option<String> },
        EscArmed(bool),
        PasteAvailable(bool),
        SetModel(String),
    }

    struct Mock {
        calls: Vec<Call>,
        clock: Vec<Instant>,
        clock_idx: usize,
        inject: InjectOutcome,
        // Optional replacement override; None = identity.
        replaced: Option<String>,
        // Last take metadata passed to append_history.
        last_meta: Option<TakeMeta>,
    }

    impl Mock {
        fn new(base: Instant) -> Self {
            Self {
                calls: Vec::new(),
                clock: vec![base],
                clock_idx: 0,
                inject: InjectOutcome::Injected { chars: 0, method: InjectMethod::Pasted },
                replaced: None,
                last_meta: None,
            }
        }
        fn with_clock(base: Instant, offsets_ms: &[u64]) -> Self {
            let mut m = Mock::new(base);
            m.clock = offsets_ms
                .iter()
                .map(|&o| base + Duration::from_millis(o))
                .collect();
            m
        }
        fn huds(&self) -> Vec<String> {
            self.calls
                .iter()
                .filter_map(|c| match c {
                    Call::Hud(s) => Some(s.clone()),
                    _ => None,
                })
                .collect()
        }
        fn has(&self, c: &Call) -> bool {
            self.calls.contains(c)
        }
    }

    fn tag(ev: &HudEvent) -> String {
        match ev {
            HudEvent::Levels { bars } => format!("levels:{}", bars.len()),
            HudEvent::State { s } => match s {
                HudState::Hidden => "hidden".into(),
                HudState::LoadingModel { pct } => format!("loading:{pct}"),
                HudState::Listening => "listening".into(),
                HudState::Transcribing => "transcribing".into(),
                HudState::Injected { chars } => format!("injected:{chars}"),
                HudState::Cancelled => "cancelled".into(),
                HudState::Error { label, detail } => format!("error:{label} / {detail}"),
                HudState::ConfirmDiscard => "confirm".into(),
            },
        }
    }

    impl Effects for Mock {
        fn start_capture(&mut self, device: Option<String>) {
            self.calls.push(Call::StartCapture(device));
        }
        fn stop_capture(&mut self) {
            self.calls.push(Call::StopCapture);
        }
        fn abort_capture(&mut self) {
            self.calls.push(Call::AbortCapture);
        }
        fn play_cue(&mut self, cue: CueKind) {
            self.calls.push(Call::Cue(cue));
        }
        fn decode(&mut self, generation: u64, samples: Vec<f32>) {
            self.calls.push(Call::Decode { gen: generation, len: samples.len() });
        }
        fn ensure_model(&mut self) {
            self.calls.push(Call::EnsureModel);
        }
        fn unload_model(&mut self) {
            self.calls.push(Call::UnloadModel);
        }
        fn inject(&mut self, text: String, _target_hwnd: isize) -> InjectOutcome {
            self.calls.push(Call::Inject(text));
            self.inject.clone()
        }
        fn capture_foreground(&mut self) -> isize {
            self.calls.push(Call::CaptureForeground);
            0x1234
        }
        fn hud(&mut self, ev: HudEvent) {
            self.calls.push(Call::Hud(tag(&ev)));
        }
        fn show_overlay(&mut self) {
            self.calls.push(Call::ShowOverlay);
        }
        fn hide_overlay(&mut self) {
            self.calls.push(Call::HideOverlay);
        }
        fn set_tray_recording(&mut self, rec: bool) {
            self.calls.push(Call::Tray(rec));
        }
        fn set_tray_error(&mut self) {
            self.calls.push(Call::TrayError);
        }
        fn toast(&mut self, msg: String) {
            self.calls.push(Call::Toast(msg));
        }
        fn foreground_exe(&mut self, _hwnd: isize) -> Option<String> {
            Some("editor.exe".into())
        }
        fn append_history(&mut self, raw: String, text: String, exe: Option<String>, meta: TakeMeta) {
            self.calls.push(Call::History { raw, text, exe });
            self.last_meta = Some(meta);
        }
        fn apply_replacements(&mut self, raw: &str, _target_hwnd: isize) -> String {
            self.replaced.clone().unwrap_or_else(|| raw.to_string())
        }
        fn set_esc_armed(&mut self, armed: bool) {
            self.calls.push(Call::EscArmed(armed));
        }
        fn set_paste_available(&mut self, on: bool) {
            self.calls.push(Call::PasteAvailable(on));
        }
        fn set_model(&mut self, id: String) {
            self.calls.push(Call::SetModel(id));
        }
        fn now(&mut self) -> Instant {
            let v = self.clock[self.clock_idx.min(self.clock.len() - 1)];
            if self.clock_idx < self.clock.len() {
                self.clock_idx += 1;
            }
            v
        }
    }

    fn cfg(mode: HotkeyMode) -> Config {
        Config { hotkey_mode: mode, ..Default::default() }
    }

    // Drive a decode round-trip: audio delivers samples, ASR answers.
    fn samples(n: usize) -> Vec<f32> {
        vec![0.1; n]
    }

    #[test]
    fn happy_path_hold_release_decode_inject() {
        let base = Instant::now();
        let mut fx = Mock::with_clock(base, &[0, 600]); // down @0, up @600 (>400 = hold)
        let mut c = Coordinator::new(cfg(HotkeyMode::Both));
        fx.inject = InjectOutcome::Injected { chars: 5, method: InjectMethod::Pasted };

        c.handle(CoordMsg::HotkeyDown, &mut fx);
        assert!(fx.has(&Call::ShowOverlay));
        assert!(fx.has(&Call::Tray(true)));
        assert!(fx.has(&Call::EscArmed(true)));
        assert!(fx.has(&Call::StartCapture(None)));

        c.handle(CoordMsg::CaptureStarted, &mut fx);
        assert!(fx.has(&Call::Cue(CueKind::Start)));
        assert_eq!(fx.huds().last().unwrap(), "listening");

        // A mid-hold segment closes -> dispatched to ASR at the session gen.
        c.handle(CoordMsg::SegmentClosed(samples(16000)), &mut fx);
        let g = c.gen;
        assert!(fx.has(&Call::Decode { gen: g, len: 16000 }));
        c.handle(CoordMsg::DecodeDone { generation: g, text: "hello".into() }, &mut fx);

        // Release -> stop cue, then tail decode.
        c.handle(CoordMsg::HotkeyUp, &mut fx);
        assert!(fx.has(&Call::StopCapture));
        assert!(fx.has(&Call::Cue(CueKind::Stop)));
        c.handle(CoordMsg::TailSegment(samples(8000)), &mut fx);
        assert_eq!(fx.huds().last().unwrap(), "transcribing");
        c.handle(CoordMsg::DecodeDone { generation: g, text: "world".into() }, &mut fx);

        // Combined "hello world" injected, history appended, HUD Injected.
        assert!(fx.has(&Call::Inject("hello world".into())));
        // Focused exe is resolved from target_hwnd and recorded (PLAN §9).
        assert!(fx.has(&Call::History {
            raw: "hello world".into(),
            text: "hello world".into(),
            exe: Some("editor.exe".into()),
        }));
        assert_eq!(fx.huds().last().unwrap(), "injected:5");
        assert!(fx.has(&Call::Tray(false)));
        assert!(fx.has(&Call::EscArmed(false)));

        // HUD-hide timer fires -> Hidden broadcast (webview fades, §2.4), then
        // the second phase actually hides the native window.
        c.fire_timer(&mut fx);
        assert_eq!(fx.huds().last().unwrap(), "hidden");
        assert!(!fx.has(&Call::HideOverlay));
        c.fire_timer(&mut fx);
        assert!(fx.has(&Call::HideOverlay));
    }

    #[test]
    fn tap_toggles_on_and_off() {
        let base = Instant::now();
        let mut fx = Mock::with_clock(base, &[0, 200]); // up @200 (<400 = tap)
        let mut c = Coordinator::new(cfg(HotkeyMode::Both));

        c.handle(CoordMsg::HotkeyDown, &mut fx);
        c.handle(CoordMsg::CaptureStarted, &mut fx);
        c.handle(CoordMsg::HotkeyUp, &mut fx); // tap -> latch, still recording
        assert!(matches!(c.state, State::Recording { toggled: true, .. }));
        assert!(!fx.has(&Call::StopCapture)); // no stop on the latching tap

        // Second press stops it.
        c.handle(CoordMsg::HotkeyDown, &mut fx);
        assert!(fx.has(&Call::StopCapture));
        assert!(matches!(c.state, State::AwaitingTail));
    }

    #[test]
    fn hold_mode_ignores_tap_latch() {
        let base = Instant::now();
        let mut fx = Mock::with_clock(base, &[0, 200]); // quick tap
        let mut c = Coordinator::new(cfg(HotkeyMode::Hold));

        c.handle(CoordMsg::HotkeyDown, &mut fx);
        c.handle(CoordMsg::HotkeyUp, &mut fx);
        // Hold mode: even a tap stops rather than latching.
        assert!(matches!(c.state, State::AwaitingTail));
        assert!(fx.has(&Call::StopCapture));
    }

    #[test]
    fn cancel_in_recording_aborts() {
        let base = Instant::now();
        let mut fx = Mock::with_clock(base, &[0, 5]); // 5s elapsed (<30)
        let mut c = Coordinator::new(cfg(HotkeyMode::Both));

        c.handle(CoordMsg::HotkeyDown, &mut fx);
        c.handle(CoordMsg::Cancel, &mut fx);
        assert!(fx.has(&Call::AbortCapture));
        assert!(fx.has(&Call::Cue(CueKind::Discard)));
        assert_eq!(fx.huds().last().unwrap(), "cancelled");
        assert!(matches!(c.state, State::Idle));
    }

    #[test]
    fn cancel_mid_decoding_drops_stale_result() {
        let base = Instant::now();
        let mut fx = Mock::with_clock(base, &[0, 600]);
        let mut c = Coordinator::new(cfg(HotkeyMode::Hold));

        c.handle(CoordMsg::HotkeyDown, &mut fx);
        let g = c.gen;
        c.handle(CoordMsg::HotkeyUp, &mut fx);
        c.handle(CoordMsg::TailSegment(samples(8000)), &mut fx);
        assert!(matches!(c.state, State::Decoding));

        // Cancel bumps generation.
        c.handle(CoordMsg::Cancel, &mut fx);
        assert!(matches!(c.state, State::Idle));
        assert_ne!(c.gen, g);

        // The stale tail result arrives — must be dropped, no inject.
        c.handle(CoordMsg::DecodeDone { generation: g, text: "ghost".into() }, &mut fx);
        assert!(!fx.has(&Call::Inject("ghost".into())));
    }

    #[test]
    fn release_before_model_ready_queues_then_decodes() {
        let base = Instant::now();
        let mut fx = Mock::with_clock(base, &[0, 600]);
        let mut c = Coordinator::new(cfg(HotkeyMode::Hold));
        c.model_status = ModelStatus::Loading { pct: 40 };

        c.handle(CoordMsg::HotkeyDown, &mut fx);
        assert!(fx.has(&Call::StartCapture(None))); // capture starts anyway (no lost words)
        assert_eq!(fx.huds().last().unwrap(), "loading:40");
        c.handle(CoordMsg::CaptureStarted, &mut fx);
        c.handle(CoordMsg::SegmentClosed(samples(16000)), &mut fx);
        // Queued, not dispatched yet.
        assert!(!fx.calls.iter().any(|x| matches!(x, Call::Decode { .. })));

        c.handle(CoordMsg::HotkeyUp, &mut fx);
        c.handle(CoordMsg::TailSegment(samples(8000)), &mut fx);
        assert!(matches!(c.state, State::Decoding));
        // Still no decode dispatched (model not ready).
        assert!(!fx.calls.iter().any(|x| matches!(x, Call::Decode { .. })));

        // Model becomes ready -> both queued chunks flush and decode.
        c.handle(CoordMsg::ModelStatus(ModelStatus::Ready), &mut fx);
        let g = c.gen;
        assert!(fx.has(&Call::Decode { gen: g, len: 16000 }));
        assert!(fx.has(&Call::Decode { gen: g, len: 8000 }));

        c.handle(CoordMsg::DecodeDone { generation: g, text: "a".into() }, &mut fx);
        c.handle(CoordMsg::DecodeDone { generation: g, text: "b".into() }, &mut fx);
        assert!(fx.has(&Call::Inject("a b".into())));
    }

    #[test]
    fn model_missing_at_down_shows_error_no_capture() {
        let base = Instant::now();
        let mut fx = Mock::new(base);
        let mut c = Coordinator::new(cfg(HotkeyMode::Both));
        c.model_status = ModelStatus::Missing;

        c.handle(CoordMsg::HotkeyDown, &mut fx);
        assert_eq!(fx.huds().last().unwrap(), "error:NO MODEL / OPEN DICTUM TO DOWNLOAD");
        assert!(fx.has(&Call::Cue(CueKind::Error)));
        assert!(!fx.calls.iter().any(|x| matches!(x, Call::StartCapture(_))));
        assert!(matches!(c.state, State::Idle));
    }

    #[test]
    fn capture_dead_decodes_buffered_tail() {
        let base = Instant::now();
        let mut fx = Mock::with_clock(base, &[0]);
        let mut c = Coordinator::new(cfg(HotkeyMode::Hold));

        c.handle(CoordMsg::HotkeyDown, &mut fx);
        let g = c.gen;
        c.handle(CoordMsg::SegmentClosed(samples(16000)), &mut fx);
        c.handle(CoordMsg::DecodeDone { generation: g, text: "buffered".into() }, &mut fx);

        // Mid-recording death: the worker flushes the tail (finalize) BEFORE
        // CaptureDead, while we're still Recording. The tail must be decoded, not
        // dropped (PLAN §4.5 — unplug must not lose audio silently).
        c.handle(CoordMsg::TailSegment(samples(8000)), &mut fx);
        c.handle(CoordMsg::CaptureDead("unplugged".into()), &mut fx);
        assert!(fx.has(&Call::Cue(CueKind::Error)));
        c.handle(CoordMsg::DecodeDone { generation: g, text: "tail".into() }, &mut fx);
        // Both the pre-death segment AND the death-path tail are injected.
        assert!(fx.has(&Call::Inject("buffered tail".into())));
    }

    #[test]
    fn capture_dead_with_no_audio_shows_no_microphone() {
        let base = Instant::now();
        let mut fx = Mock::with_clock(base, &[0]);
        let mut c = Coordinator::new(cfg(HotkeyMode::Hold));

        c.handle(CoordMsg::HotkeyDown, &mut fx);
        // Device dies before any speech was captured/decoded -> NO MICROPHONE
        // (DESIGN §1.2 verbatim) + tray error, not a misleading DISCARDED.
        c.handle(CoordMsg::CaptureDead("no input device".into()), &mut fx);
        assert!(fx.has(&Call::Cue(CueKind::Error)));
        assert_eq!(fx.huds().last().unwrap(), "error:NO MICROPHONE / PLUG IN OR PICK ANOTHER INPUT");
        assert!(fx.has(&Call::TrayError));
        assert!(!fx.calls.iter().any(|x| matches!(x, Call::Inject(_))));
        assert!(matches!(c.state, State::Idle));
    }

    #[test]
    fn decode_failure_surfaces_error_and_drops_partial() {
        let base = Instant::now();
        let mut fx = Mock::with_clock(base, &[0, 600]);
        let mut c = Coordinator::new(cfg(HotkeyMode::Hold));

        c.handle(CoordMsg::HotkeyDown, &mut fx);
        let g = c.gen;
        // One segment decoded fine, then the tail's decode fails.
        c.handle(CoordMsg::SegmentClosed(samples(16000)), &mut fx);
        c.handle(CoordMsg::DecodeDone { generation: g, text: "good".into() }, &mut fx);
        c.handle(CoordMsg::HotkeyUp, &mut fx);
        c.handle(CoordMsg::TailSegment(samples(8000)), &mut fx);
        c.handle(CoordMsg::DecodeFailed { generation: g, error: "boom".into() }, &mut fx);

        // No partial inject of the successful segment; an honest error instead.
        assert!(!fx.calls.iter().any(|x| matches!(x, Call::Inject(_))));
        assert_eq!(fx.huds().last().unwrap(), "error:DECODE FAILED / NOTHING PRINTED — TRY AGAIN");
        assert!(fx.has(&Call::Cue(CueKind::Error)));
        assert!(matches!(c.state, State::Idle));
    }

    #[test]
    fn awaiting_tail_segment_is_decoded_not_dropped() {
        // A VAD segment that closes in-flight can land after HotkeyUp (AwaitingTail)
        // but before the tail — it must still be decoded (PLAN §1.3, no lost words).
        let base = Instant::now();
        let mut fx = Mock::with_clock(base, &[0, 600]);
        let mut c = Coordinator::new(cfg(HotkeyMode::Hold));

        c.handle(CoordMsg::HotkeyDown, &mut fx);
        let g = c.gen;
        c.handle(CoordMsg::CaptureStarted, &mut fx);
        c.handle(CoordMsg::HotkeyUp, &mut fx); // -> AwaitingTail
        assert!(matches!(c.state, State::AwaitingTail));
        c.handle(CoordMsg::SegmentClosed(samples(16000)), &mut fx); // late segment in AwaitingTail
        c.handle(CoordMsg::TailSegment(samples(8000)), &mut fx); // -> Decoding
        assert!(matches!(c.state, State::Decoding));
        c.handle(CoordMsg::DecodeDone { generation: g, text: "a".into() }, &mut fx);
        c.handle(CoordMsg::DecodeDone { generation: g, text: "b".into() }, &mut fx);
        // The AwaitingTail segment was decoded and concatenated, not dropped.
        assert!(fx.has(&Call::Inject("a b".into())));
    }

    #[test]
    fn unload_on_idle_ensures_model_on_start() {
        let base = Instant::now();
        let mut fx = Mock::new(base);
        let mut c = Coordinator::new(Config {
            hotkey_mode: HotkeyMode::Hold,
            unload_on_idle: true,
            ..Default::default()
        });
        // Model not yet resident -> start must warm it (R4 / PLAN §10.3).
        c.model_status = ModelStatus::Loading { pct: 0 };
        c.handle(CoordMsg::HotkeyDown, &mut fx);
        assert!(fx.has(&Call::EnsureModel));
    }

    #[test]
    fn unload_on_idle_unloads_after_hide() {
        let base = Instant::now();
        let mut fx = Mock::new(base);
        let mut c = Coordinator::new(Config {
            hotkey_mode: HotkeyMode::Hold,
            unload_on_idle: true,
            ..Default::default()
        });
        // Model Ready by default; run the happy path to an inject (arms HideHud).
        c.handle(CoordMsg::HotkeyDown, &mut fx);
        let g = c.gen;
        c.handle(CoordMsg::CaptureStarted, &mut fx);
        c.handle(CoordMsg::SegmentClosed(samples(16000)), &mut fx);
        c.handle(CoordMsg::DecodeDone { generation: g, text: "hi".into() }, &mut fx);
        c.handle(CoordMsg::HotkeyUp, &mut fx);
        c.handle(CoordMsg::TailSegment(samples(8000)), &mut fx);
        c.handle(CoordMsg::DecodeDone { generation: g, text: "there".into() }, &mut fx);
        assert!(fx.has(&Call::Inject("hi there".into())));

        // HideHud fires (fade), then HideWindow -> overlay hides AND the
        // idle-unload timer arms.
        c.fire_timer(&mut fx);
        c.fire_timer(&mut fx);
        assert!(fx.has(&Call::HideOverlay));
        assert!(!fx.has(&Call::UnloadModel)); // not yet — only armed
        // UnloadIdle fires -> model is dropped.
        c.fire_timer(&mut fx);
        assert!(fx.has(&Call::UnloadModel));
    }

    #[test]
    fn no_unload_when_idle_unload_disabled() {
        let base = Instant::now();
        let mut fx = Mock::new(base);
        let mut c = Coordinator::new(cfg(HotkeyMode::Hold)); // unload_on_idle=false default

        c.handle(CoordMsg::HotkeyDown, &mut fx);
        let g = c.gen;
        c.handle(CoordMsg::HotkeyUp, &mut fx);
        c.handle(CoordMsg::TailSegment(samples(8000)), &mut fx);
        c.handle(CoordMsg::DecodeDone { generation: g, text: "x".into() }, &mut fx);

        c.fire_timer(&mut fx); // HideHud -> fade
        c.fire_timer(&mut fx); // HideWindow -> hide, but no idle-unload armed
        assert!(fx.has(&Call::HideOverlay));
        c.fire_timer(&mut fx); // nothing armed
        assert!(!fx.has(&Call::UnloadModel));
    }

    #[test]
    fn empty_transcript_is_discarded() {
        let base = Instant::now();
        let mut fx = Mock::with_clock(base, &[0, 600]);
        let mut c = Coordinator::new(cfg(HotkeyMode::Hold));

        c.handle(CoordMsg::HotkeyDown, &mut fx);
        let g = c.gen;
        c.handle(CoordMsg::HotkeyUp, &mut fx);
        c.handle(CoordMsg::TailSegment(samples(8000)), &mut fx);
        c.handle(CoordMsg::DecodeDone { generation: g, text: "   ".into() }, &mut fx);

        assert_eq!(fx.huds().last().unwrap(), "cancelled");
        assert!(!fx.calls.iter().any(|x| matches!(x, Call::Inject(_))));
    }

    #[test]
    fn long_recording_esc_confirms_then_discards() {
        let base = Instant::now();
        // down @0, first Cancel @31s (>30 -> confirm), second Cancel needs no clock read.
        let mut fx = Mock::with_clock(base, &[0, 31_000]);
        let mut c = Coordinator::new(cfg(HotkeyMode::Hold));

        c.handle(CoordMsg::HotkeyDown, &mut fx);
        c.handle(CoordMsg::Cancel, &mut fx);
        assert_eq!(fx.huds().last().unwrap(), "confirm");
        assert!(c.confirm_pending);
        assert!(!fx.has(&Call::AbortCapture)); // not discarded yet
        assert!(matches!(c.state, State::Recording { .. }));

        // Second Esc within the window discards.
        c.handle(CoordMsg::Cancel, &mut fx);
        assert!(fx.has(&Call::AbortCapture));
        assert!(matches!(c.state, State::Idle));
    }

    #[test]
    fn confirm_discard_reverts_on_timeout() {
        let base = Instant::now();
        let mut fx = Mock::with_clock(base, &[0, 31_000]);
        let mut c = Coordinator::new(cfg(HotkeyMode::Hold));

        c.handle(CoordMsg::HotkeyDown, &mut fx);
        c.handle(CoordMsg::CaptureStarted, &mut fx);
        c.handle(CoordMsg::Cancel, &mut fx);
        assert!(c.confirm_pending);

        c.fire_timer(&mut fx); // RevertConfirm
        assert!(!c.confirm_pending);
        assert_eq!(fx.huds().last().unwrap(), "listening");
        assert!(matches!(c.state, State::Recording { .. }));
    }

    #[test]
    fn focus_changed_holds_text_and_paste_last_reinjects() {
        let base = Instant::now();
        let mut fx = Mock::with_clock(base, &[0, 600]);
        let mut c = Coordinator::new(cfg(HotkeyMode::Hold));
        fx.inject = InjectOutcome::FocusChanged;

        c.handle(CoordMsg::HotkeyDown, &mut fx);
        let g = c.gen;
        c.handle(CoordMsg::HotkeyUp, &mut fx);
        c.handle(CoordMsg::TailSegment(samples(8000)), &mut fx);
        c.handle(CoordMsg::DecodeDone { generation: g, text: "held text".into() }, &mut fx);

        assert!(fx
            .huds()
            .iter()
            .any(|h| h.contains("WINDOW CHANGED / SENT TO CLIPBOARD — PASTE IT")));
        assert_eq!(c.last_text.as_deref(), Some("held text"));
        assert!(matches!(c.state, State::Idle));
        assert!(fx.has(&Call::PasteAvailable(true)));

        // PasteLast now succeeds into the refocused window.
        fx.inject = InjectOutcome::Injected { chars: 9, method: InjectMethod::Pasted };
        c.handle(CoordMsg::PasteLast, &mut fx);
        assert!(fx.has(&Call::Inject("held text".into())));
        assert_eq!(fx.huds().last().unwrap(), "injected:9");
    }

    #[test]
    fn elevated_target_shows_exact_copy() {
        let base = Instant::now();
        let mut fx = Mock::with_clock(base, &[0, 600]);
        let mut c = Coordinator::new(cfg(HotkeyMode::Hold));
        fx.inject = InjectOutcome::ElevatedClipboardOnly;

        c.handle(CoordMsg::HotkeyDown, &mut fx);
        let g = c.gen;
        c.handle(CoordMsg::HotkeyUp, &mut fx);
        c.handle(CoordMsg::TailSegment(samples(8000)), &mut fx);
        c.handle(CoordMsg::DecodeDone { generation: g, text: "x".into() }, &mut fx);

        assert_eq!(fx.huds().last().unwrap(), "error:PROTECTED WINDOW / SENT TO CLIPBOARD — PASTE IT");
        assert!(fx.has(&Call::Cue(CueKind::Error)));
    }

    #[test]
    fn model_switch_reloads_recognizer() {
        let base = Instant::now();
        let mut fx = Mock::new(base);
        let mut c = Coordinator::new(cfg(HotkeyMode::Hold));
        let mut new_cfg = cfg(HotkeyMode::Hold);
        new_cfg.model_id = "parakeet-tdt-0.6b-v3-int8".into();
        c.handle(CoordMsg::ConfigChanged(new_cfg.clone()), &mut fx);
        assert!(fx.has(&Call::SetModel("parakeet-tdt-0.6b-v3-int8".into())));
        assert!(fx.has(&Call::EnsureModel)); // warm the new SKU (default config)
        // Same config again: no redundant switch.
        fx.calls.clear();
        c.handle(CoordMsg::ConfigChanged(new_cfg), &mut fx);
        assert!(!fx.calls.iter().any(|x| matches!(x, Call::SetModel(_))));
    }

    #[test]
    fn model_switch_with_unload_on_idle_stays_cold() {
        let base = Instant::now();
        let mut fx = Mock::new(base);
        let mut c = Coordinator::new(cfg(HotkeyMode::Hold));
        let mut new_cfg = cfg(HotkeyMode::Hold);
        new_cfg.model_id = "parakeet-tdt-0.6b-v3-int8".into();
        new_cfg.unload_on_idle = true;
        c.handle(CoordMsg::ConfigChanged(new_cfg), &mut fx);
        assert!(fx.has(&Call::SetModel("parakeet-tdt-0.6b-v3-int8".into())));
        assert!(!fx.has(&Call::EnsureModel)); // lazy-loads on the next take
    }

    #[test]
    fn config_change_swaps_mode_live() {
        let base = Instant::now();
        let mut fx = Mock::with_clock(base, &[0, 200]);
        let mut c = Coordinator::new(cfg(HotkeyMode::Hold));
        c.handle(CoordMsg::ConfigChanged(cfg(HotkeyMode::Both)), &mut fx);

        // Now a tap should latch (Both), proving the swap took effect.
        c.handle(CoordMsg::HotkeyDown, &mut fx);
        c.handle(CoordMsg::HotkeyUp, &mut fx);
        assert!(matches!(c.state, State::Recording { toggled: true, .. }));
    }

    #[test]
    fn levels_forwarded_while_recording() {
        let base = Instant::now();
        let mut fx = Mock::new(base);
        let mut c = Coordinator::new(cfg(HotkeyMode::Hold));
        c.handle(CoordMsg::HotkeyDown, &mut fx);
        c.handle(CoordMsg::Levels(vec![LevelBar { amp: 0.5, clip: false }]), &mut fx);
        assert!(fx.huds().iter().any(|h| h == "levels:1"));
    }

    #[test]
    fn take_metadata_recorded_with_history() {
        let base = Instant::now();
        let mut fx = Mock::with_clock(base, &[0, 600]);
        let mut c = Coordinator::new(cfg(HotkeyMode::Hold));
        fx.inject = InjectOutcome::Injected { chars: 4, method: InjectMethod::Typed };

        c.handle(CoordMsg::HotkeyDown, &mut fx);
        let g = c.gen;
        c.handle(CoordMsg::CaptureStarted, &mut fx);
        // 4 bars = 150ms of audio, one clipped.
        c.handle(
            CoordMsg::Levels(vec![
                LevelBar { amp: 0.2, clip: false },
                LevelBar { amp: 0.9, clip: true },
                LevelBar { amp: 0.4, clip: false },
                LevelBar { amp: 0.1, clip: false },
            ]),
            &mut fx,
        );
        c.handle(CoordMsg::HotkeyUp, &mut fx);
        c.handle(CoordMsg::TailSegment(samples(8000)), &mut fx);
        c.handle(CoordMsg::DecodeDone { generation: g, text: "test".into() }, &mut fx);

        let meta = fx.last_meta.as_ref().expect("history appended with meta");
        assert_eq!(meta.dur_ms, 150); // 4 × 37.5ms
        assert!(meta.clipped);
        assert_eq!(meta.envelope, vec![0.2, 0.9, 0.4, 0.1]);
        assert_eq!(meta.method, Some(InjectMethod::Typed));
        assert!(fx.has(&Call::PasteAvailable(true)));
    }

    #[test]
    fn downsample_envelope_preserves_peaks() {
        // Short series passes through untouched.
        assert_eq!(downsample_envelope(&[0.1, 0.2], 64), vec![0.1, 0.2]);
        // 8 -> 4 buckets of 2, each keeping its max.
        let out = downsample_envelope(&[0.1, 0.9, 0.2, 0.3, 0.8, 0.1, 0.0, 0.5], 4);
        assert_eq!(out, vec![0.9, 0.3, 0.8, 0.5]);
        // Length is capped at `max`.
        let long: Vec<f32> = (0..1000).map(|i| (i % 10) as f32 / 10.0).collect();
        assert_eq!(downsample_envelope(&long, 64).len(), 64);
    }

    #[test]
    fn unloaded_model_warms_on_start() {
        let base = Instant::now();
        let mut fx = Mock::new(base);
        let mut c = Coordinator::new(Config {
            hotkey_mode: HotkeyMode::Hold,
            unload_on_idle: true,
            ..Default::default()
        });
        // asr announced the unload; the next take must warm the model.
        c.handle(CoordMsg::ModelStatus(ModelStatus::Unloaded), &mut fx);
        c.handle(CoordMsg::HotkeyDown, &mut fx);
        assert!(fx.has(&Call::EnsureModel));
        assert!(fx.has(&Call::StartCapture(None))); // capture starts in parallel
    }
}

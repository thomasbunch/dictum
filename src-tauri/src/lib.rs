//! Dictum — Tauri integration layer. Wires the coordinator state machine to the
//! real subsystems (audio, ASR, inject, history, cues, overlay, tray, HUD) and
//! owns the command surface + app lifecycle.

pub mod types;

mod asr;
mod audio;
mod commands;
mod config;
mod coordinator;
mod history;
mod hotkey;
mod inject;
mod model;
mod overlay;
mod replacements;
mod tray;

use std::sync::{mpsc::Sender, Arc, Mutex};
use std::time::Instant;
use tauri::{ipc::Channel, AppHandle, Emitter, Manager};

use crate::audio::Cue;
use crate::coordinator::{CueKind, Effects};
use crate::tray::TrayState;
use crate::types::{Config, CoordMsg, HudEvent, InjectOutcome, ModelStatus};

/// Fan-out of HUD events to every subscribed webview (overlay + Settings level
/// lane). Dead channels are dropped on the next broadcast.
#[derive(Clone, Default)]
pub struct HudBroadcaster(Arc<Mutex<Vec<Channel<HudEvent>>>>);

impl HudBroadcaster {
    pub fn subscribe(&self, ch: Channel<HudEvent>) {
        self.0.lock().unwrap().push(ch);
    }
    pub fn broadcast(&self, ev: &HudEvent) {
        self.0.lock().unwrap().retain(|c| c.send(ev.clone()).is_ok());
    }
}

/// Shared state for commands. `Mutex<Sender>` (not a bare Sender) so the struct
/// is `Sync` for `app.manage`.
pub struct AppState {
    pub config: Arc<Mutex<Config>>,
    pub coord_tx: Mutex<Sender<CoordMsg>>,
    pub hud: HudBroadcaster,
    pub history: Arc<Mutex<history::History>>,
    /// Shared with the coordinator's `Effects` (esc-arm) and the resume detector
    /// (re-arm); `set_config` rebinds through it.
    pub hotkey: Arc<Mutex<hotkey::HotkeyManager>>,
}

/// The concrete `Effects` the coordinator calls out through. Keeps coordinator.rs
/// free of Tauri/Win32 calls (it can be unit-tested against a mock Effects).
struct RealEffects {
    app: AppHandle,
    hud: HudBroadcaster,
    tray: tray::Tray,
    cues: audio::Cues,
    audio: audio::AudioPipeline,
    asr: asr::AsrEngine,
    history: Arc<Mutex<history::History>>,
    config: Arc<Mutex<Config>>,
    hotkey: Arc<Mutex<hotkey::HotkeyManager>>,
}

/// Coordinator's earcon enum -> the audio module's `Cue`.
fn cue_to_audio(k: CueKind) -> Cue {
    match k {
        CueKind::Start => Cue::Start,
        CueKind::Stop => Cue::Stop,
        CueKind::Discard => Cue::Discard,
        CueKind::Error => Cue::Error,
    }
}

impl Effects for RealEffects {
    fn start_capture(&mut self, device: Option<String>) {
        self.audio.start(device);
    }
    fn stop_capture(&mut self) {
        self.audio.stop();
    }
    fn abort_capture(&mut self) {
        self.audio.abort();
    }
    fn play_cue(&mut self, cue: CueKind) {
        self.cues.play(cue_to_audio(cue));
    }
    fn decode(&mut self, generation: u64, samples: Vec<f32>) {
        self.asr.decode(generation, samples);
    }
    fn ensure_model(&mut self) {
        self.asr.ensure_loaded();
    }
    fn unload_model(&mut self) {
        self.asr.unload();
    }
    fn inject(&mut self, text: String, target_hwnd: isize) -> InjectOutcome {
        let cfg = self.config.lock().unwrap().clone();
        crate::inject::inject(&text, target_hwnd, &cfg)
    }
    fn capture_foreground(&mut self) -> isize {
        foreground_hwnd()
    }
    fn hud(&mut self, ev: HudEvent) {
        // Visibility is driven by the coordinator via show_overlay/hide_overlay;
        // here we only fan the event out to subscribed webviews.
        self.hud.broadcast(&ev);
    }
    fn show_overlay(&mut self) {
        crate::overlay::position_and_show(&self.app);
    }
    fn hide_overlay(&mut self) {
        crate::overlay::hide(&self.app);
    }
    fn set_tray_recording(&mut self, rec: bool) {
        self.tray
            .set_state(if rec { TrayState::Recording } else { TrayState::Idle });
    }
    fn set_tray_error(&mut self) {
        self.tray.set_state(TrayState::Error);
    }
    fn toast(&mut self, msg: String) {
        // No dedicated toast window in v1 (FocusChanged / ClipboardManual hold the
        // text for PasteLast). Surface as an app event so a future listener can
        // render it, plus stderr for dev visibility — never silent.
        // ponytail: event-only sink; add a toast UI when a consumer exists.
        eprintln!("toast: {msg}");
        let _ = self.app.emit("toast", msg);
    }
    fn foreground_exe(&mut self, hwnd: isize) -> Option<String> {
        crate::inject::exe_for_hwnd(hwnd)
    }
    fn append_history(&mut self, raw: String, text: String, exe: Option<String>) {
        // append() no-ops internally when keep_transcripts is off / retention is
        // KeepNothing. exe is resolved from the target window (PLAN §9).
        let cfg = self.config.lock().unwrap().clone();
        let _ = self
            .history
            .lock()
            .unwrap()
            .append(&raw, &text, exe.as_deref(), &cfg);
    }
    fn apply_replacements(&mut self, raw: &str) -> String {
        let cfg = self.config.lock().unwrap().clone();
        crate::replacements::apply(raw, &cfg)
    }
    fn set_esc_armed(&mut self, armed: bool) {
        let _ = self.hotkey.lock().unwrap().arm_esc(armed);
    }
    fn now(&mut self) -> Instant {
        Instant::now()
    }
}

pub fn run() {
    tauri::Builder::default()
        // single-instance MUST be registered first (RESEARCH tauri §7).
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(w) = app.get_webview_window("settings") {
                let _ = w.show();
                let _ = w.set_focus();
            }
        }))
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .invoke_handler(tauri::generate_handler![
            commands::get_config,
            commands::set_config,
            commands::try_hotkey,
            commands::list_input_devices,
            commands::model_info,
            commands::download_model,
            commands::history_list,
            commands::history_delete,
            commands::history_undo_delete,
            commands::history_meta,
            commands::paste_last,
            commands::import_replacements,
            commands::export_replacements,
            commands::subscribe_hud,
            commands::copy_text,
        ])
        .setup(|app| {
            let handle = app.handle().clone();
            overlay::setup(&handle);

            let config = Arc::new(Mutex::new(config::load()));
            let init_cfg = config.lock().unwrap().clone();
            let (tx, rx) = std::sync::mpsc::channel::<CoordMsg>();

            // Bundled files keep their config-relative "resources/" prefix under
            // resource_dir (verified: exe-dir/resources/ in dev and bundle).
            let res_dir = handle.path().resource_dir()?.join("resources");
            let vad_path = res_dir.join("silero_vad.onnx");

            let audio = audio::AudioPipeline::new(tx.clone(), vad_path);
            let asr = asr::AsrEngine::new(tx.clone());
            let cues = audio::Cues::new(&res_dir, init_cfg.audio_cues);
            let history = Arc::new(Mutex::new(
                history::History::open(&init_cfg).expect("open history.db"),
            ));
            let hud = HudBroadcaster::default();
            let tray = tray::init(&handle, tx.clone(), &init_cfg)?;

            // Register the hotkey now; surface failure — never silent (PLAN §1.4).
            // On failure the manager is still built (unbound) so the app runs and
            // Settings can rebind to a free chord.
            let hotkey_mgr = match hotkey::HotkeyManager::register(
                handle.clone(),
                &init_cfg.hotkey,
                tx.clone(),
            ) {
                Ok(m) => m,
                Err(e) => {
                    eprintln!("hotkey register failed: {e}");
                    let _ = handle.emit("hotkey://error", &init_cfg.hotkey);
                    hotkey::HotkeyManager::unbound(handle.clone(), &init_cfg.hotkey, tx.clone())
                }
            };
            let hotkey = Arc::new(Mutex::new(hotkey_mgr));

            app.manage(AppState {
                config: config.clone(),
                coord_tx: Mutex::new(tx.clone()),
                hud: hud.clone(),
                history: history.clone(),
                hotkey: hotkey.clone(),
            });

            // Warm the model unless it's missing (missing -> first hotkey shows
            // MODEL NOT FOUND, Settings shows GET). ensure_loaded() is a non-
            // blocking channel send; the asr worker thread does the real load.
            if model::model_files().all_present() {
                asr.ensure_loaded();
            } else if let Some(archive) = model::find_dropped_archive() {
                // Offline sideload (PLAN §4.4): a hand-dropped .tar.bz2 in the
                // models dir installs on a background thread (600 MB extract),
                // then warm-loads.
                let asr2 = asr.clone();
                let tx2 = tx.clone();
                std::thread::spawn(move || match model::install_from_archive(&archive) {
                    Ok(()) => asr2.ensure_loaded(),
                    Err(e) => {
                        eprintln!("sideload install failed: {e}");
                        let _ = tx2.send(CoordMsg::ModelStatus(ModelStatus::Missing));
                    }
                });
            } else {
                let _ = tx.send(CoordMsg::ModelStatus(ModelStatus::Missing));
            }

            let mut fx = RealEffects {
                app: handle.clone(),
                hud,
                tray,
                cues,
                audio,
                asr,
                history,
                config,
                hotkey: hotkey.clone(),
            };
            std::thread::spawn(move || {
                coordinator::Coordinator::run(rx, &mut fx, init_cfg);
            });

            spawn_resume_detector(hotkey);
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Dictum");
}

/// Foreground window handle at call time (captured at hotkey-down, passed back
/// to `inject`). inject.rs keeps its GetForegroundWindow use internal, so the
/// capture point lives here.
fn foreground_hwnd() -> isize {
    use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
    unsafe { GetForegroundWindow() }.0 as isize
}

/// Detect sleep/resume with zero Win32: a suspended thread's `thread::sleep`
/// still consumes real wall-clock time, so a gap far larger than the poll
/// interval means the machine slept — re-arm the hotkey (R7, Handy #1620).
/// ponytail: wall-clock-gap heuristic instead of WTSRegisterSessionNotification/
/// WM_POWERBROADCAST WndProc subclassing (RESEARCH win §). Swap in the win32
/// path only if the heuristic proves flaky.
fn spawn_resume_detector(hotkey: Arc<Mutex<hotkey::HotkeyManager>>) {
    use std::time::{Duration, SystemTime};
    std::thread::spawn(move || {
        let interval = Duration::from_secs(20);
        loop {
            let before = SystemTime::now();
            std::thread::sleep(interval);
            let slept = before.elapsed().unwrap_or(interval);
            if slept > interval + Duration::from_secs(20) {
                // The global-shortcut hook can silently die across sleep — re-arm.
                if let Ok(hk) = hotkey.lock() {
                    let _ = hk.rearm();
                }
            }
        }
    });
}

#[cfg(test)]
mod e2e {
    //! Real-hardware pipeline test (M0 acceptance): downloads the model via the
    //! app's own network path if absent (~628 MB, resumable), then decodes the
    //! archive's bundled test WAV and measures RTF.
    //! Run: cargo test --release -- --ignored e2e

    #[test]
    #[ignore]
    fn model_download_and_decode() {
        use crate::types::DownloadProgress;
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::time::Instant;

        let files = crate::model::model_files();
        if !files.all_present() {
            eprintln!("model absent — downloading via model::download()");
            let failed = AtomicBool::new(false);
            crate::model::download(|p| match &p {
                DownloadProgress::Progress { pct, mb_done, mb_total } => {
                    if pct % 5 == 0 {
                        eprintln!("  {pct}% ({mb_done}/{mb_total} MB)");
                    }
                }
                DownloadProgress::Verifying => eprintln!("  verifying SHA256…"),
                DownloadProgress::Done => eprintln!("  done"),
                DownloadProgress::Failed { error } => {
                    eprintln!("  FAILED: {error}");
                    failed.store(true, Ordering::SeqCst);
                }
            });
            assert!(!failed.load(Ordering::SeqCst), "model download failed");
        }
        let files = crate::model::model_files();
        assert!(files.all_present(), "model files missing after download");

        // Same construction as asr.rs (kept in sync by the review suite).
        let mut cfg = sherpa_onnx::OfflineRecognizerConfig::default();
        cfg.model_config.transducer = sherpa_onnx::OfflineTransducerModelConfig {
            encoder: Some(files.encoder.to_string_lossy().into_owned()),
            decoder: Some(files.decoder.to_string_lossy().into_owned()),
            joiner: Some(files.joiner.to_string_lossy().into_owned()),
        };
        cfg.model_config.tokens = Some(files.tokens.to_string_lossy().into_owned());
        cfg.model_config.provider = Some("cpu".into());
        cfg.model_config.num_threads = 4;

        let load_start = Instant::now();
        let rec = sherpa_onnx::OfflineRecognizer::create(&cfg).expect("recognizer create");
        eprintln!("model load: {:.1}s", load_start.elapsed().as_secs_f32());

        let wav = crate::model::model_dir().join("test_wavs").join("0.wav");
        assert!(wav.exists(), "bundled test wav missing: {}", wav.display());
        let wave =
            sherpa_onnx::Wave::read(wav.to_string_lossy().as_ref()).expect("read test wav");
        let audio_secs = wave.num_samples() as f32 / wave.sample_rate() as f32;

        let t = Instant::now();
        let stream = rec.create_stream();
        stream.accept_waveform(wave.sample_rate(), wave.samples());
        rec.decode(&stream);
        let text = stream.get_result().map(|r| r.text).unwrap_or_default();
        let decode = t.elapsed().as_secs_f32();
        let rtf = decode / audio_secs;

        eprintln!("transcript: {text}");
        eprintln!("audio {audio_secs:.2}s  decode {decode:.3}s  RTF {rtf:.4}");
        assert!(!text.trim().is_empty(), "empty transcript from test wav");
        assert!(rtf < 1.0, "RTF {rtf} ≥ 1.0 — misses the latency budget");
    }
}

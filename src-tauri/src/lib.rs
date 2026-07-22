//! Dictum — Tauri integration layer. Wires the coordinator state machine to the
//! real subsystems (audio, ASR, inject, history, cues, overlay, tray, HUD) and
//! owns the command surface + app lifecycle.

pub mod types;

mod asr;
mod audio;
mod commands;
mod config;
mod coordinator;
mod filetag;
mod gpu;
mod guardrail;
mod history;
mod hotkey;
mod inject;
mod model;
mod overlay;
mod reformat;
mod replacements;
mod tray;
mod voice;

use std::sync::{mpsc::Sender, Arc, Mutex};
use std::time::Instant;
use tauri::{ipc::Channel, AppHandle, Emitter, Manager};

use crate::audio::Cue;
use crate::coordinator::{CueKind, Effects};
use crate::tray::TrayState;
use crate::types::{
    Config, CoordMsg, GpuInfoDto, HudEvent, HudState, InjectOutcome, ModelStatus, ModelStatusDto, TakeMeta,
};

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
    /// Last announced model status (SETUP model card reads it at boot; live
    /// updates arrive on the `model://status` event).
    pub model_status: Arc<Mutex<ModelStatusDto>>,
    /// Reformat (LLM) model status — parallel to `model_status`, never conflated
    /// with the ASR engine. Live updates on `reformat://status`.
    pub reformat_status: Arc<Mutex<ModelStatusDto>>,
    /// GPU capability probed once at startup (SETUP reformatter section).
    pub gpu: GpuInfoDto,
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
    /// LLM reformatter worker (own thread; llama ctx is !Send). Fire-and-forget.
    reformat: reformat::ReformatEngine,
    /// The GPU-gated reformat SKU this session uses (path + presence check).
    reformat_spec: &'static model::ModelSpec,
    history: Arc<Mutex<history::History>>,
    config: Arc<Mutex<Config>>,
    hotkey: Arc<Mutex<hotkey::HotkeyManager>>,
    model_status: Arc<Mutex<ModelStatusDto>>,
    reformat_status: Arc<Mutex<ModelStatusDto>>,
    /// FILE TAG index over config.project_roots; rebuilt in the background at
    /// every session start (capture_foreground), read at apply_replacements.
    file_index: Arc<Mutex<filetag::Index>>,
    /// Last click-through value pushed to the overlay (avoid a main-thread hop
    /// per HUD state when nothing changed).
    overlay_click_through: bool,
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
        // Refresh the FILE TAG index while the take records — the walk is
        // ms-scale and decode is seconds away. Lives here (not in
        // capture_foreground) so PasteLast never triggers a pointless walk.
        // ponytail: rebuild-per-session, no watcher; add notify-debounce only
        // if huge roots make this measurably late.
        let roots = self.config.lock().unwrap().project_roots.clone();
        if !roots.is_empty() {
            let idx = self.file_index.clone();
            std::thread::spawn(move || {
                let built = filetag::Index::build(&roots);
                *idx.lock().unwrap() = built;
            });
        }
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
        // DESIGN §5.6: the HUD is click-through EXCEPT during confirm_discard
        // and error. Toggle only on an actual change (main-thread hop).
        if let HudEvent::State { s } = &ev {
            let click_through =
                !matches!(s, HudState::ConfirmDiscard | HudState::Error { .. });
            if click_through != self.overlay_click_through {
                self.overlay_click_through = click_through;
                crate::overlay::set_click_through(&self.app, click_through);
            }
        }
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
    fn append_history(&mut self, raw: String, text: String, exe: Option<String>, meta: TakeMeta) {
        // append() no-ops internally when keep_transcripts is off / retention is
        // KeepNothing. exe is resolved from the target window (PLAN §9).
        let cfg = self.config.lock().unwrap().clone();
        let _ = self
            .history
            .lock()
            .unwrap()
            .append(&raw, &text, exe.as_deref(), &meta, &cfg);
        // The main window (if open) prints the new line on arrival (§5.2).
        let _ = self.app.emit("history://changed", ());
    }
    fn apply_replacements(&mut self, raw: &str, target_hwnd: isize) -> String {
        let cfg = self.config.lock().unwrap().clone();
        // Deterministic pipeline order (PLAN §6): voice commands FIRST (they edit
        // the spoken transcript), then replacements/snippets, then file tagging.
        let voiced = crate::voice::apply(raw);
        // ponytail: {cursor} caret placement is reserved, not wired. apply() strips
        // the sentinel so text integrity is correct; the caret offset (from
        // apply_with_cursor) is dropped here on purpose. Positioning it means firing
        // N LEFT arrows after inject, which the inject path can't do reliably —
        // Ctrl+V paste completes asynchronously (arrows would race the paste), the
        // backend varies per app, and the elevated / focus-changed paths are
        // clipboard-only with no caret at all. Wire it (thread the offset through
        // Effects::inject) only once inject gains a synchronous paste-complete signal.
        let text = crate::replacements::apply(&voiced, &cfg);
        if cfg.project_roots.is_empty() {
            return text;
        }
        let title = filetag::window_title(target_hwnd);
        filetag::apply(&text, &self.file_index.lock().unwrap(), title.as_deref())
    }
    fn set_esc_armed(&mut self, armed: bool) {
        let _ = self.hotkey.lock().unwrap().arm_esc(armed);
    }
    fn announce_model_status(&mut self, st: &ModelStatus) {
        let dto = ModelStatusDto::from(st);
        *self.model_status.lock().unwrap() = dto.clone();
        let _ = self.app.emit("model://status", dto);
    }
    fn set_paste_available(&mut self, on: bool) {
        self.tray.set_paste_enabled(on);
    }
    fn set_model(&mut self, id: String) {
        self.asr.set_model(id);
    }
    // --- Reformat LLM ------------------------------------------------------
    fn reformat(&mut self, det: String, generation: u64) {
        self.reformat.reformat(det, generation);
    }
    fn ensure_reformat_model(&mut self) {
        self.reformat.ensure();
    }
    fn unload_reformat_model(&mut self) {
        self.reformat.unload();
    }
    fn set_reformat_model(&mut self, id: String) {
        // No config field selects the reformat SKU (it's GPU-gated at startup), so
        // this is rarely called; kept for the Effects contract. Repoint the worker.
        self.reformat_spec = model::reformat_spec(&id);
        self.reformat.set_model(model::single_file_path(self.reformat_spec));
    }
    fn reformat_model_present(&mut self) -> bool {
        model::files_present(self.reformat_spec)
    }
    fn announce_reformat_status(&mut self, st: &ModelStatus) {
        let dto = ModelStatusDto::from(st);
        *self.reformat_status.lock().unwrap() = dto.clone();
        let _ = self.app.emit("reformat://status", dto);
    }
    fn now(&mut self) -> Instant {
        Instant::now()
    }
}

pub fn run() {
    tauri::Builder::default()
        // single-instance MUST be registered first (RESEARCH tauri §7).
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.unminimize();
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
            commands::history_count,
            commands::paste_last,
            commands::toggle_dictation,
            commands::get_model_status,
            commands::get_reformat_status,
            commands::get_gpu_info,
            commands::import_replacements,
            commands::export_replacements,
            commands::subscribe_hud,
            commands::copy_text,
        ])
        .setup(|app| {
            let handle = app.handle().clone();
            overlay::setup(&handle);

            // Dictum is a tray app: closing the main window hides it, never quits.
            if let Some(main) = app.get_webview_window("main") {
                let mw = main.clone();
                main.on_window_event(move |e| {
                    if let tauri::WindowEvent::CloseRequested { api, .. } = e {
                        api.prevent_close();
                        let _ = mw.hide();
                    }
                });
            }

            let config = Arc::new(Mutex::new(config::load()));
            let init_cfg = config.lock().unwrap().clone();
            let (tx, rx) = std::sync::mpsc::channel::<CoordMsg>();

            // Bundled files keep their config-relative "resources/" prefix under
            // resource_dir (verified: exe-dir/resources/ in dev and bundle).
            let res_dir = handle.path().resource_dir()?.join("resources");
            let vad_path = res_dir.join("silero_vad.onnx");

            let audio = audio::AudioPipeline::new(tx.clone(), vad_path);
            let asr = asr::AsrEngine::new(tx.clone(), init_cfg.model_id.clone());

            // GPU probe (once) decides the reformat SKU: 3B on a capable dGPU,
            // else 1.5B CPU. The worker points at the SKU's GGUF but loads it
            // lazily on the first reformat — never at boot (2-4 GB).
            let gpu_info = gpu::probe();
            let gpu_dto = GpuInfoDto { vram_mb: gpu_info.vram_mb, offer_gpu_3b: gpu_info.offer_gpu_3b };
            // 3B auto-pick requires the vulkan build; CPU-only builds cap at 1.5B.
            // Without the vulkan feature reformat.rs sets n_gpu_layers=0, so a 3B
            // picked on VRAM alone runs entirely on CPU (~4-13s) — the exact
            // regression the soft-gate exists to avoid. Explicit reformat="on"
            // still honors the GPU pick so a user can deliberately override.
            let offer_3b =
                gpu_info.offer_gpu_3b && (cfg!(feature = "vulkan") || init_cfg.reformat == "on");
            let reformat_spec = model::reformat_spec(model::reformat_id_for_gpu(offer_3b));
            let reformat = reformat::ReformatEngine::new(tx.clone());
            // The worker is pointed at the SKU below via fx.set_reformat_model once
            // fx exists — startup and config-swap share the same Effects seam.
            // Offline sideload: install a hand-dropped reformat .gguf if present.
            // Install only (no load) so the reformatter stays lazy.
            std::thread::spawn(model::sideload_reformat_gguf);

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
            let active_spec = model::spec(&init_cfg.model_id);
            let model_status = Arc::new(Mutex::new(if model::model_files(active_spec).all_present() {
                ModelStatusDto::Loading { pct: 0 }
            } else {
                ModelStatusDto::Missing
            }));
            // Reformat model is lazy: present-on-disk reads as Unloaded (not Ready)
            // until the first reformat loads it; absent reads as Missing.
            let reformat_status = Arc::new(Mutex::new(if model::files_present(reformat_spec) {
                ModelStatusDto::Unloaded
            } else {
                ModelStatusDto::Missing
            }));

            app.manage(AppState {
                config: config.clone(),
                coord_tx: Mutex::new(tx.clone()),
                hud: hud.clone(),
                history: history.clone(),
                hotkey: hotkey.clone(),
                model_status: model_status.clone(),
                reformat_status: reformat_status.clone(),
                gpu: gpu_dto,
            });

            // Warm the active model unless it's missing (missing -> first hotkey
            // shows MODEL NOT FOUND, Settings shows GET). ensure_loaded() is a
            // non-blocking channel send; the asr worker thread does the real load.
            if model::model_files(active_spec).all_present() {
                asr.ensure_loaded();
            } else if let Some(archive) = model::find_dropped_archive() {
                // Offline sideload (PLAN §4.4): a hand-dropped .tar.bz2 in the
                // models dir installs on a background thread (600 MB extract) —
                // the archive's checksums decide which SKU it is. Warm-load only
                // if it turned out to be the active one.
                let asr2 = asr.clone();
                let tx2 = tx.clone();
                let active_id = init_cfg.model_id.clone();
                std::thread::spawn(move || match model::install_from_archive(&archive) {
                    Ok(spec) if spec.id == active_id => asr2.ensure_loaded(),
                    Ok(_) => {} // other SKU installed; SETUP shows it present
                    Err(e) => {
                        eprintln!("sideload install failed: {e}");
                        let _ = tx2.send(CoordMsg::ModelStatus(ModelStatus::Missing));
                    }
                });
            } else {
                let _ = tx.send(CoordMsg::ModelStatus(ModelStatus::Missing));
            }

            // FILE TAG index: warm it once at boot so the first take can tag.
            let file_index = Arc::new(Mutex::new(filetag::Index::default()));
            if !init_cfg.project_roots.is_empty() {
                let idx = file_index.clone();
                let roots = init_cfg.project_roots.clone();
                std::thread::spawn(move || *idx.lock().unwrap() = filetag::Index::build(&roots));
            }

            let mut fx = RealEffects {
                app: handle.clone(),
                hud,
                tray,
                cues,
                audio,
                asr,
                reformat,
                reformat_spec,
                history,
                config,
                hotkey: hotkey.clone(),
                model_status,
                reformat_status,
                file_index,
                overlay_click_through: true, // overlay::setup made it click-through
            };
            // Point the reformat worker at the GPU-gated SKU (lazy-loaded on first
            // use). Routed through the Effects seam so startup and any config swap
            // share one path.
            fx.set_reformat_model(reformat_spec.id.into());
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

        let spec = crate::model::spec(crate::model::DEFAULT_MODEL_ID);
        let files = crate::model::model_files(spec);
        if !files.all_present() {
            eprintln!("model absent — downloading via model::download()");
            let failed = AtomicBool::new(false);
            crate::model::download(spec, |p| match &p {
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
        let files = crate::model::model_files(spec);
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

        let wav = crate::model::model_dir(spec).join("test_wavs").join("0.wav");
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

    /// Multilingual SKU (Parakeet v3): decode the archive's bundled de/en/es/fr
    /// WAVs through the registry path — proves model switch + auto language
    /// detection end to end. Requires the v3 model installed (download via
    /// SETUP or sideload); skips with a message when absent.
    /// Run: cargo test --release -- --ignored v3_multilingual
    #[test]
    #[ignore]
    fn v3_multilingual_decode() {
        use std::time::Instant;

        let spec = crate::model::spec("parakeet-tdt-0.6b-v3-int8");
        let files = crate::model::model_files(spec);
        if !files.all_present() {
            eprintln!("v3 model not installed — skipping (fetch it via SETUP first)");
            return;
        }

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
        let rec = sherpa_onnx::OfflineRecognizer::create(&cfg).expect("v3 recognizer create");
        eprintln!("v3 load: {:.1}s", load_start.elapsed().as_secs_f32());

        for lang in ["de", "en", "es", "fr"] {
            let wav = crate::model::model_dir(spec).join("test_wavs").join(format!("{lang}.wav"));
            assert!(wav.exists(), "bundled {lang}.wav missing: {}", wav.display());
            let wave = sherpa_onnx::Wave::read(wav.to_string_lossy().as_ref()).expect("read wav");
            let audio_secs = wave.num_samples() as f32 / wave.sample_rate() as f32;

            let t = Instant::now();
            let stream = rec.create_stream();
            stream.accept_waveform(wave.sample_rate(), wave.samples());
            rec.decode(&stream);
            let text = stream.get_result().map(|r| r.text).unwrap_or_default();
            let rtf = t.elapsed().as_secs_f32() / audio_secs;

            eprintln!("[{lang}] RTF {rtf:.4} — {text}");
            assert!(!text.trim().is_empty(), "empty transcript for {lang}.wav");
            assert!(rtf < 1.0, "[{lang}] RTF {rtf} ≥ 1.0 — misses the latency budget");
        }
    }
}

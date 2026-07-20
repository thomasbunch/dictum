# CONTRACTS.md — module boundaries and wiring (build-time doc)

Read PLAN.md (architecture, locked decisions) and DESIGN.md (all UI) first.
`src-tauri/src/types.rs` is the compile-enforced shared contract — import from
it, never edit it. If your module genuinely needs a change to types.rs or a new
Cargo dependency, append one line to `NEEDS.md` (create if absent) and code
against what you proposed; the orchestrator reconciles.

## File ownership (strict — write only your files)

| Agent | Owns |
|---|---|
| audio | `src-tauri/src/audio/{mod,capture,resample,vad,cues}.rs` |
| asr | `src-tauri/src/{asr,model}.rs` |
| coord | `src-tauri/src/{coordinator,hotkey}.rs` |
| inject | `src-tauri/src/inject/{mod,clipboard,sendinput,integrity,overrides}.rs` |
| support | `src-tauri/src/{history,replacements,config}.rs` |
| shell | `src-tauri/src/{main,lib,commands,overlay,tray}.rs`, `src-tauri/build.rs`, icons, earcons, `src-tauri/resources/*` |
| hud | `src/overlay/*` |
| ui | `src/settings/*`, `src/history/*`, `src/shared.ts` |
| orchestrator | `types.rs`, `Cargo.toml`, `tauri.conf.json`, `package.json`, `vite.config.ts`, `src/tokens.css`, `src/bindings.ts`, `*.html` |

## Threading (PLAN.md §3) and channel wiring

- Coordinator runs on its own thread; single `std::sync::mpsc::Receiver<CoordMsg>`.
  Everyone else holds a `Sender<CoordMsg>` clone. Coordinator never blocks on
  anything but its receiver (use `recv_timeout` for timed states).
- Audio: `AudioPipeline::new(coord_tx, vad_model_path)` spawns its worker thread
  once. `start(device)` opens the cpal stream (callback thread cpal-owned; the
  callback only pushes into a lock-free ring buffer). Worker drains, downmixes,
  resamples 48k→16k (rubato), runs Silero VAD live, sends `CaptureStarted`,
  `Levels`, `SegmentClosed` (VAD-closed mid-hold), and on `stop()` sends
  `TailSegment`. `abort()` discards. Stream error callback → `CaptureDead`.
  VAD config: threshold 0.3, min_silence 0.5 s, min_speech 0.1 s, max_speech 30 s.
- ASR: `AsrEngine` owns the warm `OfflineRecognizer` on its own thread; requests
  via its own channel `(generation, samples)`, replies with
  `CoordMsg::DecodeDone/DecodeFailed{generation}`. Coordinator bumps generation
  on cancel — stale results are dropped by generation mismatch, never by timing.
  Greedy search only. Model load at app start (unless `unload_on_idle`).
- `model.rs`: presence/SHA256 check, first-run download (resumable, ureq or per
  research), sideload validation, `ModelInfo`. Emits `CoordMsg::ModelStatus`.
- Cues: pre-decoded samples, dedicated output thread, `play(cue)` is fire-and-
  forget and can never block or fail loudly (Handy #1712).
- Inject: pure function `inject(text, target_hwnd, &config) -> InjectOutcome`,
  called from the coordinator thread. Steps: verify foreground HWND unchanged →
  integrity pre-check (elevated → `ElevatedClipboardOnly`) → wait for physical
  modifier release (GetAsyncKeyState loop, 2 s timeout) → backend per override
  table (clipboard-swap default: snapshot text-only, write CF_UNICODETEXT +
  exclusion formats, Ctrl+V via SendInput, restore after 300 ms off-thread;
  SendInput KEYEVENTF_UNICODE fallback in ~64-unit chunks).
- History: rusqlite at `%APPDATA%\Dictum\history.db`, WAL. Respect retention on
  open and after each append. `keep_transcripts=false` → append is a no-op.
- Replacements: `apply(text, &config) -> String` — deterministic, case-
  insensitive word-boundary replacement + optional filler removal (um/uh).
  Runs between ASR and inject. No LLM.
- Hotkey: register chord from config via tauri-plugin-global-shortcut; forward
  raw Pressed→`HotkeyDown`, Released→`HotkeyUp`. Tap-vs-hold and toggle logic
  live in the coordinator (tap = release within 400 ms ⇒ toggle latch when mode
  allows). Esc registered only while a session is active → `Cancel`. Re-register
  on `SystemResumed`. Registration failure → surfaced, never silent.

## Coordinator state machine (explicit edges, all others = ignore + debug log)

States: `Idle`, `Recording{toggled, started_at}`, `AwaitingTail`, `Decoding{generation}`,
`Injecting`. Transient HUD-only states (Injected/Cancelled/Error) are timed
hide events, not machine states.

- Idle + HotkeyDown → Recording (audio.start; HUD Listening on CaptureStarted;
  HUD LoadingModel if model not Ready)
- Recording + HotkeyUp: held < 400 ms and mode∈{Toggle,Both} → stay (toggled);
  else → AwaitingTail (audio.stop, stop cue now — before decode)
- Recording(toggled) + (HotkeyDown or ToggleDictation) → AwaitingTail
- Recording + Cancel → Idle (audio.abort, discard cue, HUD Cancelled;
  if elapsed > 30 s first Cancel → ConfirmDiscard for 2 s)
- AwaitingTail + TailSegment → Decoding (send tail + prior segment texts wait)
- Decoding + DecodeDone(gen match) → Injecting → inject() → HUD outcome → Idle
- Decoding + Cancel → Idle (generation bump; HUD Cancelled)
- any + CaptureDead → decode what's buffered if in Recording, else Idle + error cue
- any + ModelStatus/ConfigChanged/SystemResumed → update, re-arm as needed
- Injecting outcome FocusChanged → toast + hold text as "last"; PasteLast
  re-injects into the now-focused window.

## Frontend

Three windows (labels: `overlay`, `settings`, `history`), three Vite HTML
entries. All styling from `src/tokens.css` variables — zero hex literals in
window CSS (grep-able rule: `oxide` only in the waveform painter). Theme attr:
`document.documentElement.dataset.field = theme`. IBM Plex fonts self-hosted in
`src/fonts/` (woff2, no network).

- overlay: canvas chart-recorder per DESIGN.md §2 (320×64, pen head at 75%,
  80 px/s audio-clocked, ink-dry 120 ms, clip bars oxide full-height forever).
  Subscribes once via `subscribe_hud(Channel<HudEvent>)`. Elapsed = bar count ×
  37.5 ms. No DOM animation loops at idle — rAF runs only while bars arrive.
- settings/history: per DESIGN.md §4/§5. Native `<select>`, flat controls, 8px
  grid, two sizes/two weights per surface.

## Tauri commands (shell agent, `commands.rs`)

`get_config() -> Config`, `set_config(Config) -> Result<(), String>` (persists,
broadcasts ConfigChanged), `try_hotkey(chord) -> Result<(), String>`,
`list_input_devices() -> Vec<String>`, `model_info() -> Vec<ModelInfo>`,
`download_model(id, Channel<DownloadProgress>)`, `history_list(search: Option<String>)
-> Vec<HistoryRecord>`, `history_delete(id)`, `history_undo_delete()`,
`history_meta() -> String` (footer line), `paste_last()`,
`import_replacements(text, format: "txt"|"json") -> Result<u32, String>`,
`export_replacements(format) -> String`, `subscribe_hud(Channel<HudEvent>)`.

## Non-negotiables enforced everywhere

- No network code outside `model.rs` download path. No telemetry. No updater.
- Overlay never takes focus (focus:false + WS_EX_NOACTIVATE + click-through).
- Nothing animates at idle (DESIGN.md §7 idle audit).
- Oxide = captured signal only.
- Every failure mode surfaces user-visible copy from DESIGN.md §1.2 verbatim.
- Microcopy: instrument voice, tracked caps, mono numerals.

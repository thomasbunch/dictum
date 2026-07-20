//! Shared types — the compile-enforced contract between all modules.
//! Owned by the orchestrator. Module agents import from here and do not edit.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// Coordinator messages — every input the state machine can receive.
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum CoordMsg {
    // Hotkey / user intent
    HotkeyDown,
    HotkeyUp,
    /// Esc pressed while a session is active.
    Cancel,
    /// Tray left-click or menu "Start/Stop dictation".
    ToggleDictation,
    PasteLast,

    // Audio pipeline (audio module -> coordinator)
    /// First real frames arrived after start() — triggers start cue + LISTENING.
    CaptureStarted,
    /// VAD closed a speech segment mid-hold (16 kHz mono f32).
    SegmentClosed(Vec<f32>),
    /// Open tail segment delivered after stop() (may be empty).
    TailSegment(Vec<f32>),
    /// Amplitude bars for the HUD lane (one per BAR_SAMPLES samples).
    Levels(Vec<LevelBar>),
    /// Capture stream died (device unplug etc.). Decode what is buffered.
    CaptureDead(String),

    // ASR (asr module -> coordinator)
    DecodeDone { generation: u64, text: String },
    DecodeFailed { generation: u64, error: String },
    ModelStatus(ModelStatus),

    // System
    /// WM_POWERBROADCAST resume / session unlock — re-arm hotkey.
    SystemResumed,
    ConfigChanged(Config),
    Shutdown,
}

/// One amplitude bar for the chart-recorder lane.
/// One bar per `BAR_SAMPLES` input samples; the frontend derives elapsed time
/// from bar count (audio-clocked scroll, DESIGN.md §2.2).
#[derive(Debug, Clone, Copy, Serialize)]
pub struct LevelBar {
    /// Peak amplitude 0.0..=1.0 for the bar window.
    pub amp: f32,
    /// Sample in window reached >= -1 dBFS — bar prints oxide, full height, forever.
    pub clip: bool,
}

/// 16 kHz mono samples per HUD bar: 80 px/s scroll, 3 px per bar (2px bar+1px gap)
/// => 26.67 bars/s => 600 samples.
pub const BAR_SAMPLES: usize = 600;

#[derive(Debug, Clone, PartialEq)]
pub enum ModelStatus {
    Missing,
    Loading { pct: u8 },
    Ready,
    Error(String),
}

// ---------------------------------------------------------------------------
// HUD protocol (Rust -> overlay webview over tauri::ipc::Channel)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum HudEvent {
    /// Full state transition. The overlay window is shown/hidden by Rust;
    /// the webview only paints.
    State { s: HudState },
    /// Bars appended since the last event (audio-clocked).
    Levels { bars: Vec<LevelBar> },
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "k", rename_all = "snake_case")]
pub enum HudState {
    Hidden,
    LoadingModel { pct: u8 },
    Listening,
    /// Paper stops; elapsed frozen client-side.
    Transcribing,
    Injected { chars: u32 },
    Cancelled,
    /// `msg` is the exact tracked-caps copy from DESIGN.md §1.2.
    Error { msg: String },
    /// Recording > 30 s and Esc pressed once: "ESC AGAIN TO DISCARD".
    ConfirmDiscard,
}

// ---------------------------------------------------------------------------
// Config — %APPDATA%\Dictum\config.json. Serde defaults keep first run zero-config.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct Config {
    /// Chord in tauri-plugin-global-shortcut syntax, e.g. "Ctrl+Alt+D".
    /// The global-shortcut backend (global-hotkey) requires a non-modifier main
    /// key, so modifier-only combos like "Ctrl+Super" are NOT registerable —
    /// bare-modifier PTT is deferred to the win-hotkeys backend (PLAN §121, v1.1).
    /// Ctrl+Alt+Space was rejected: the Claude desktop app registers it for its
    /// own dictation (verified colliding on a real machine).
    pub hotkey: String,
    pub hotkey_mode: HotkeyMode,
    /// None = system default input device.
    pub input_device: Option<String>,
    pub audio_cues: bool,
    /// Unload model after idle (reload 1-3 s on next use). Default: always loaded.
    pub unload_on_idle: bool,
    pub theme: Theme,
    pub keep_transcripts: bool,
    pub retention: Retention,
    pub vocabulary: Vec<String>,
    pub replacements: Vec<Replacement>,
    pub remove_fillers: bool,
    /// exe name (lowercase) -> per-app injection override.
    pub app_overrides: std::collections::BTreeMap<String, AppOverride>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            hotkey: "Ctrl+Alt+D".into(),
            hotkey_mode: HotkeyMode::Both,
            input_device: None,
            audio_cues: true,
            unload_on_idle: false,
            theme: Theme::Ledger,
            keep_transcripts: true,
            retention: Retention::Days7,
            vocabulary: Vec::new(),
            replacements: Vec::new(),
            remove_fillers: false,
            app_overrides: default_app_overrides(),
        }
    }
}

/// RDP/Citrix clients default to SendInput unicode (clipboard redirection is
/// commonly GPO-disabled); terminals default to Ctrl+Shift+V paste.
pub fn default_app_overrides() -> std::collections::BTreeMap<String, AppOverride> {
    let mut m = std::collections::BTreeMap::new();
    for exe in ["mstsc.exe", "msrdc.exe", "wfica32.exe"] {
        m.insert(exe.into(), AppOverride { backend: Some(InjectBackend::SendInputUnicode), ..Default::default() });
    }
    m.insert("windowsterminal.exe".into(), AppOverride { paste_shortcut: Some(PasteShortcut::CtrlShiftV), ..Default::default() });
    m
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum HotkeyMode { Hold, Toggle, Both }

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "UPPERCASE")]
pub enum Theme { Ledger, Bone, Plaster, Glacier, Lilac, Obsidian }

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Retention { KeepNothing, Hours24, Days7, Days30, Forever }

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Replacement {
    /// Matched case-insensitively on word boundaries.
    pub heard: String,
    pub printed: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct AppOverride {
    pub backend: Option<InjectBackend>,
    pub paste_shortcut: Option<PasteShortcut>,
    /// ms between SendInput chunks (default 5).
    pub chunk_delay_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum InjectBackend { Clipboard, SendInputUnicode }

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum PasteShortcut { CtrlV, CtrlShiftV }

// ---------------------------------------------------------------------------
// Injection
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum InjectOutcome {
    /// Text landed via clipboard-paste or SendInput fallback.
    Injected { chars: u32 },
    /// Target elevated (UIPI): text copied to clipboard only.
    ElevatedClipboardOnly,
    /// Foreground window changed since hotkey-down: text held, not pasted.
    FocusChanged,
    /// All backends failed: text is on the clipboard, paste manually.
    ClipboardManual(String),
}

// ---------------------------------------------------------------------------
// History
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryRecord {
    pub id: i64,
    /// Unix millis.
    pub ts: i64,
    /// Raw ASR output before replacements.
    pub raw: String,
    /// Final injected text.
    pub text: String,
    /// Focused exe name at injection, e.g. "chrome.exe" (future per-app modes).
    pub exe: Option<String>,
}

// ---------------------------------------------------------------------------
// Model management
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelInfo {
    pub id: String,          // "parakeet-tdt-0.6b-v2-int8"
    pub display: String,     // "PARAKEET-TDT 0.6B V2 INT8"
    pub present: bool,
    pub size_mb: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum DownloadProgress {
    Progress { pct: u8, mb_done: u64, mb_total: u64 },
    Verifying,
    Done,
    Failed { error: String },
}

pub fn models_dir() -> PathBuf {
    dirs::data_dir().expect("APPDATA").join("Dictum").join("models")
}

pub fn app_data_dir() -> PathBuf {
    dirs::data_dir().expect("APPDATA").join("Dictum")
}

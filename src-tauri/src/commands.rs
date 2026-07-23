//! Tauri command surface (CONTRACTS.md "Tauri commands"). Parameter names match
//! src/bindings.ts exactly — renaming one silently breaks the frontend call.

use tauri::ipc::Channel;
use tauri::{AppHandle, Emitter, State};

use crate::types::{
    Config, CoordMsg, DownloadProgress, GpuInfoDto, HistoryRecord, HudEvent, ModelInfo, ModelKind,
    ModelStatus, ModelStatusDto, Replacement,
};
use crate::AppState;

#[tauri::command]
pub fn get_config(state: State<AppState>) -> Config {
    state.config.lock().unwrap().clone()
}

#[tauri::command]
pub fn set_config(config: Config, app: AppHandle, state: State<AppState>) -> Result<(), String> {
    let old_hotkey = state.config.lock().unwrap().hotkey.clone();
    if config.hotkey != old_hotkey {
        // rebind() restores the old chord on failure (synchronous rollback/UX).
        state
            .hotkey
            .lock()
            .unwrap()
            .rebind(&config.hotkey)
            .map_err(|_| "Hotkey unavailable — another app may already use this combination.".to_string())?;
    }
    persist(&app, &state, config)
}

#[tauri::command]
pub fn try_hotkey(chord: String, app: AppHandle) -> Result<(), String> {
    crate::hotkey::try_hotkey(&app, &chord)
}

#[tauri::command]
pub fn list_input_devices() -> Vec<String> {
    crate::audio::list_input_devices()
}

#[tauri::command]
pub fn model_info() -> Vec<ModelInfo> {
    crate::model::MODELS.iter().map(crate::model::check).collect()
}

#[tauri::command]
pub fn download_model(id: String, progress: Channel<DownloadProgress>, state: State<AppState>) {
    let spec = crate::model::spec(&id);
    let tx = state.coord_tx.lock().unwrap().clone();
    let active = state.config.lock().unwrap().model_id == spec.id;
    let kind = spec.kind;
    // Blocking download (the app's only network path) — off the IPC thread.
    // On completion, tell the coordinator so the SETUP card refreshes without a
    // restart:
    //   - ASR active model: now usable (recognizer lazy-loads on first decode via
    //     asr::ensure). Only the ACTIVE model changes anything live.
    //   - LLM (reformat) SKU: mirror the boot 'present => Unloaded' rule so the
    //     card flips to STANDBY (an LLM id is never the active ASR model, so the
    //     `active` branch above never covers it and state.reformat_status would
    //     otherwise stay Missing until the first reformat or an app restart).
    std::thread::spawn(move || {
        crate::model::download(spec, move |p| {
            let done = matches!(p, DownloadProgress::Done);
            let _ = progress.send(p);
            if done {
                match kind {
                    ModelKind::Asr if active => {
                        let _ = tx.send(CoordMsg::ModelStatus(ModelStatus::Ready));
                    }
                    ModelKind::Llm => {
                        let _ = tx.send(CoordMsg::ReformatModelStatus { status: ModelStatus::Unloaded });
                    }
                    _ => {}
                }
            }
        });
    });
}

#[tauri::command]
pub fn history_list(search: Option<String>, state: State<AppState>) -> Vec<HistoryRecord> {
    state.history.lock().unwrap().list(search.as_deref()).unwrap_or_default()
}

#[tauri::command]
pub fn history_delete(id: i64, state: State<AppState>) {
    let _ = state.history.lock().unwrap().delete(id);
}

#[tauri::command]
pub fn history_undo_delete(state: State<AppState>) {
    let _ = state.history.lock().unwrap().undo_delete();
}

/// Total tape line count (list() is capped at 500; the toolbar meta needs all).
#[tauri::command]
pub fn history_count(state: State<AppState>) -> i64 {
    state.history.lock().unwrap().count()
}

#[tauri::command]
pub fn paste_last(state: State<AppState>) {
    let _ = state.coord_tx.lock().unwrap().send(CoordMsg::PasteLast);
}

/// Masthead keycaps: start/stop a test dictation (DESIGN §5.1).
#[tauri::command]
pub fn toggle_dictation(state: State<AppState>) {
    let _ = state.coord_tx.lock().unwrap().send(CoordMsg::ToggleDictation);
}

/// Boot-time model status for the SETUP card; live updates on `model://status`.
#[tauri::command]
pub fn get_model_status(state: State<AppState>) -> ModelStatusDto {
    state.model_status.lock().unwrap().clone()
}

/// Boot-time reformat (LLM) model status; live updates on `reformat://status`.
#[tauri::command]
pub fn get_reformat_status(state: State<AppState>) -> ModelStatusDto {
    state.reformat_status.lock().unwrap().clone()
}

/// GPU capability probed at startup — SETUP reformatter section shows which SKU
/// the gate picked (offerGpu3b => 3B, else 1.5B CPU).
#[tauri::command]
pub fn get_gpu_info(state: State<AppState>) -> GpuInfoDto {
    state.gpu.clone()
}

#[tauri::command]
pub fn import_replacements(text: String, format: String, app: AppHandle, state: State<AppState>) -> Result<u32, String> {
    let reps: Vec<Replacement> = match format.as_str() {
        "json" => serde_json::from_str(&text).map_err(|e| e.to_string())?,
        _ => text.lines().filter_map(parse_txt_line).collect(),
    };
    let n = reps.len() as u32;
    let mut cfg = state.config.lock().unwrap().clone();
    cfg.replacements = reps;
    persist(&app, &state, cfg)?;
    Ok(n)
}

#[tauri::command]
pub fn export_replacements(format: String, state: State<AppState>) -> String {
    let reps = state.config.lock().unwrap().replacements.clone();
    match format.as_str() {
        "json" => serde_json::to_string_pretty(&reps).unwrap_or_default(),
        _ => reps.iter().map(|r| format!("{} => {}", r.heard, r.printed)).collect::<Vec<_>>().join("\n"),
    }
}

#[tauri::command]
pub fn subscribe_hud(channel: Channel<HudEvent>, state: State<AppState>) {
    state.hud.subscribe(channel);
}

/// History COPY button (bindings.ts addition — see NEEDS-shell.md).
#[tauri::command]
pub fn copy_text(text: String) -> Result<(), String> {
    clipboard_win::set_clipboard_string(&text).map_err(|e| e.to_string())
}

// --- helpers ---

/// Persist config, update shared state, tell the coordinator, and re-theme every
/// window. Used by set_config and replacement import.
fn persist(app: &AppHandle, state: &AppState, cfg: Config) -> Result<(), String> {
    crate::config::save(&cfg).map_err(|e| e.to_string())?;
    *state.config.lock().unwrap() = cfg.clone();
    let _ = state.coord_tx.lock().unwrap().send(CoordMsg::ConfigChanged(cfg.clone()));
    let _ = app.emit("config://changed", &cfg);
    Ok(())
}

/// One replacement per line: `heard => printed`, `heard<TAB>printed`, or
/// `heard=printed`. Blank lines and `#` comments skipped.
fn parse_txt_line(line: &str) -> Option<Replacement> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let (h, p) = line
        .split_once("=>")
        .or_else(|| line.split_once('\t'))
        .or_else(|| line.split_once('='))?;
    let (h, p) = (h.trim(), p.trim());
    if h.is_empty() {
        return None;
    }
    Some(Replacement { heard: h.into(), printed: p.into() })
}

#[cfg(test)]
mod tests {
    use super::parse_txt_line;

    #[test]
    fn parses_arrow_tab_and_equals() {
        assert_eq!(parse_txt_line("teh => the").unwrap().printed, "the");
        assert_eq!(parse_txt_line("teh\tthe").unwrap().heard, "teh");
        assert_eq!(parse_txt_line("teh=the").unwrap().printed, "the");
    }

    #[test]
    fn skips_blanks_and_comments() {
        assert!(parse_txt_line("   ").is_none());
        assert!(parse_txt_line("# a note").is_none());
        assert!(parse_txt_line("=> nothing").is_none());
    }
}

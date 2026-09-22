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
                    ModelKind::Stream => {
                        // Mirror the LLM 'present => Unloaded' rule so the SETUP
                        // card flips to STANDBY without a restart (a Stream id is
                        // never the active ASR model, so the `active` arm above
                        // never covers it).
                        let _ = tx.send(CoordMsg::StreamModelStatus { status: ModelStatus::Unloaded });
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

/// Boot-time streaming-preview model status; live updates on `stream://status`.
#[tauri::command]
pub fn get_stream_status(state: State<AppState>) -> ModelStatusDto {
    state.stream_status.lock().unwrap().clone()
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
        _ => reps.iter().map(to_txt_line).collect::<Vec<_>>().join("\n"),
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

/// One TXT export line. Newlines are escaped so a multi-line snippet stays on
/// one physical line and re-imports intact; backslash-first so the escape is
/// reversible (a literal `\n` in the value is not confused with a newline).
fn to_txt_line(r: &Replacement) -> String {
    format!("{} => {}", esc(&r.heard), esc(&r.printed))
}

fn esc(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\n', "\\n")
}

/// Single pass so `\\n` (escaped backslash + n) stays literal, not a newline.
fn unesc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut c = s.chars();
    while let Some(ch) = c.next() {
        if ch == '\\' {
            match c.next() {
                Some('n') => out.push('\n'),
                Some('\\') => out.push('\\'),
                Some(o) => {
                    out.push('\\');
                    out.push(o);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(ch);
        }
    }
    out
}

/// One replacement per line: `heard => printed`, `heard<TAB>printed`, or
/// `heard=printed`. Blank lines and `#` comments skipped. Values are unescaped
/// (see `esc`) so a multi-line snippet round-trips.
fn parse_txt_line(line: &str) -> Option<Replacement> {
    // Strip a leading UTF-8 BOM before trimming: U+FEFF is NOT Unicode-whitespace,
    // so trim() would otherwise glue it onto the first heard key (Windows editors
    // like Notepad add a BOM on save, breaking a re-imported first rule).
    let line = line.trim_start_matches('\u{feff}').trim();
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
    Some(Replacement { heard: unesc(h), printed: unesc(p) })
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn txt_roundtrip_multiline_value() {
        // THE audit corruption case: a multi-line snippet value.
        let r = Replacement { heard: "sig".into(), printed: "Best regards,\nThomas".into() };
        let line = to_txt_line(&r);
        assert!(!line.contains('\n')); // stays one physical line
        assert!(line.contains("\\n")); // esc actually wired into export (not just the helper)
        assert_eq!(parse_txt_line(&line).unwrap(), r);
    }

    #[test]
    fn txt_roundtrip_literal_backslash_n() {
        // A value holding the literal two chars backslash+n must NOT become a
        // newline on re-import (backslash-first escaping).
        let r = Replacement { heard: "path".into(), printed: "a\\nb".into() };
        assert_eq!(parse_txt_line(&to_txt_line(&r)).unwrap(), r);
    }

    #[test]
    fn txt_first_arrow_delimits() {
        // "=>" inside the value survives: split_once splits on the FIRST arrow.
        let parsed = parse_txt_line("cmd => a => b").unwrap();
        assert_eq!(parsed.heard, "cmd");
        assert_eq!(parsed.printed, "a => b");
    }

    #[test]
    fn json_multiline_value_round_trips() {
        // Live JSON path (serde) preserves a newline in a value.
        let reps = vec![Replacement { heard: "sig".into(), printed: "Best,\nThomas".into() }];
        let s = serde_json::to_string_pretty(&reps).unwrap();
        let back: Vec<Replacement> = serde_json::from_str(&s).unwrap();
        assert_eq!(back, reps);
    }

    // --- escape identities (esc/unesc are a backslash-first pair) ---

    #[test]
    fn unesc_esc_is_identity_over_nasty_values() {
        // unesc(esc(x)) == x is the load-bearing identity for TXT export. Prove it
        // over every awkward shape: real newline, CRLF, literal backslash-n, lone
        // and paired backslashes, backslash-right-before-newline, unicode, empty.
        for x in [
            "",
            "a",
            "line1\nline2",
            "crlf\r\nend",
            "lit\\n",       // literal backslash + n
            "\\",           // lone trailing backslash
            "\\\\",         // two backslashes
            "\\\n",         // backslash immediately before a real newline
            "\n\\n\n",      // newline, literal backslash-n, newline
            "café 日本語 😀 {cursor}",
        ] {
            assert_eq!(unesc(&esc(x)), x, "unesc(esc) broke on {:?}", x);
        }
    }

    #[test]
    fn esc_is_idempotent_on_its_own_image() {
        // esc(unesc(x)) == x wherever x is already a valid escaped string (the image
        // of esc). Equivalent to esc round-trip idempotence: esc(unesc(esc(y)))==esc(y).
        for y in ["a\nb", "lit\\n", "\\\n", "plain", "\r\n"] {
            let e = esc(y);
            assert_eq!(esc(&unesc(&e)), e, "esc∘unesc not identity on image for {:?}", y);
        }
    }

    // --- property round-trip: 400 structured-random rule values, both paths ---

    fn lcg(s: &mut u64) -> u64 {
        *s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        *s >> 33
    }

    fn gen_value(s: &mut u64) -> String {
        // Fragment menu covering every hazard the format must survive.
        const FRAGS: &[&str] = &[
            "", "a", "b", "hello world", "café", "日本語テスト", "😀🎉",
            "{cursor}", "mid{cursor}dle", "\n", "\r\n", "l1\nl2\nl3",
            "\ttab", "back\\slash", "lit\\n", "\\", "\\\\", "trail ", " lead",
            "  ", "\r", "a => b", "=>", "x=>y", "#hash", "=eq", "end.\n",
        ];
        let n = (lcg(s) % 4) as usize; // 0..=3 fragments
        let mut out = String::new();
        for _ in 0..n {
            out.push_str(FRAGS[(lcg(s) as usize) % FRAGS.len()]);
        }
        if lcg(s) % 17 == 0 {
            out.push_str(&"Xy9_".repeat(2000)); // "very long value" (~8 KB)
        }
        out
    }

    /// Exact predicate for when a rep is STRICTLY lossless through TXT: the escaped
    /// forms carry no edge whitespace that trim() would eat, heard is non-empty and
    /// holds no delimiter arrow, and heard doesn't collide with the `#` comment mark.
    fn txt_lossless(r: &Replacement) -> bool {
        let eh = esc(&r.heard);
        let ep = esc(&r.printed);
        !eh.is_empty()
            && eh.trim() == eh
            && ep.trim() == ep
            && !eh.contains("=>")
            && !eh.starts_with('#')
    }

    #[test]
    fn txt_json_roundtrip_property() {
        let mut seed = 0x1234_5678_9abc_def0u64;
        let (mut total, mut core) = (0u32, 0u32);
        for _ in 0..400 {
            let rep = Replacement { heard: gen_value(&mut seed), printed: gen_value(&mut seed) };
            total += 1;

            // (A) One rep is always ONE physical line — no real newline ever leaks.
            assert!(!to_txt_line(&rep).contains('\n'), "newline leaked: {:?}", rep);

            // (B) JSON path (serde) is lossless for EVERY value.
            let js = serde_json::to_string_pretty(std::slice::from_ref(&rep)).unwrap();
            let back: Vec<Replacement> = serde_json::from_str(&js).unwrap();
            assert_eq!(back, vec![rep.clone()], "JSON lossy: {:?}", rep);

            // (C+D) TXT path: idempotent for every value, strictly lossless where
            // the escaping guarantees it, and only ever drops empty/comment heards.
            match parse_txt_line(&to_txt_line(&rep)) {
                Some(once) => {
                    let twice = parse_txt_line(&to_txt_line(&once));
                    assert_eq!(twice.as_ref(), Some(&once), "TXT not idempotent: {:?}", rep);
                    if txt_lossless(&rep) {
                        assert_eq!(once, rep, "TXT lossy for a lossless-class value: {:?}", rep);
                        core += 1;
                    }
                }
                None => {
                    // A drop is legitimate only when the heard KEY the parser would
                    // extract (text before the first "=>", trimmed) is empty, or the
                    // line is a comment. That covers all-whitespace heards and the
                    // delimiter/`#` landing at the very front — the by-design
                    // arrow-in-heard class. Guard: never drop a recoverable key.
                    let line = to_txt_line(&rep);
                    let key = line.trim().split("=>").next().unwrap_or("");
                    assert!(key.trim().is_empty() || line.trim().starts_with('#'),
                        "TXT silently dropped a recoverable rep: {:?}", rep);
                }
            }
        }
        assert!(total >= 200);
        assert!(core >= 50, "generator produced too few strictly-lossless cases: {core}");
    }

    // --- documented TXT limitations (define + assert the behavior that EXISTS) ---
    // These are by-design trade-offs, NOT bugs: JSON is the lossless path when any
    // apply. Locked so a future "fix" to the flexible parser can't silently regress
    // them unnoticed.

    #[test]
    fn txt_drops_value_edge_whitespace_json_keeps_it() {
        // trim() strips delimiter padding, which also eats a value's own edge space.
        let r = Replacement { heard: "comma".into(), printed: ", ".into() };
        assert_eq!(parse_txt_line(&to_txt_line(&r)).unwrap().printed, ","); // lost
        let js = serde_json::to_string(std::slice::from_ref(&r)).unwrap();
        let back: Vec<Replacement> = serde_json::from_str(&js).unwrap();
        assert_eq!(back[0].printed, ", "); // JSON keeps it
    }

    #[test]
    fn txt_arrow_in_heard_mis_splits_by_design() {
        // "=>" inside HEARD collides with the delimiter (first-arrow-wins, already
        // locked by txt_first_arrow_delimits). heard="a=>b" comes back as "a".
        let r = Replacement { heard: "a=>b".into(), printed: "c".into() };
        assert_eq!(parse_txt_line(&to_txt_line(&r)).unwrap().heard, "a");
    }

    #[test]
    fn txt_hash_leading_heard_is_dropped() {
        // A heard starting with '#' collides with the comment mark → dropped on import.
        let r = Replacement { heard: "#tag".into(), printed: "x".into() };
        assert!(parse_txt_line(&to_txt_line(&r)).is_none());
    }

    // --- malformed-import attack surface: must never panic/hang ---

    /// Mirror of import_replacements' TXT arm (Tauri State isn't constructible here).
    fn import_txt(text: &str) -> Vec<Replacement> {
        text.lines().filter_map(parse_txt_line).collect()
    }

    #[test]
    fn malformed_txt_import_is_robust() {
        assert!(import_txt("heard").is_empty());       // no delimiter -> skipped
        assert!(import_txt("=> printed").is_empty());  // empty heard  -> skipped
        assert!(import_txt("=>").is_empty());          // arrow only   -> skipped
        assert!(import_txt("   \n# note\n\t\n").is_empty()); // blanks + comment

        // truncated "heard =>" keeps the rule with an empty printed (not dropped).
        let r = import_txt("heard =>");
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].printed, "");

        // mixed \r\n and \n line endings both split (std lines()).
        let r = import_txt("a => 1\r\nb => 2\nc => 3");
        assert_eq!(r.iter().map(|x| x.heard.as_str()).collect::<Vec<_>>(), ["a", "b", "c"]);

        // duplicate heard keys: import keeps BOTH — dedup is the rules engine's job,
        // not the importer's. Locked so nobody "helpfully" collapses them here.
        let r = import_txt("k => 1\nk => 2");
        assert_eq!(r.len(), 2);
        assert_eq!((r[0].printed.as_str(), r[1].printed.as_str()), ("1", "2"));
    }

    #[test]
    fn bom_prefixed_txt_import_strips_bom() {
        // Regression: a leading UTF-8 BOM must not glue onto the first heard key.
        let r = import_txt("\u{feff}word => text\nsecond => y");
        assert_eq!(r[0].heard, "word");
        assert_eq!(r[0].printed, "text");
        assert_eq!(r[1].heard, "second");
    }

    #[test]
    fn huge_txt_import_terminates() {
        // 10 MB with no delimiter -> single O(n) scan, no hang, skipped.
        let big = "x".repeat(10 * 1024 * 1024);
        assert!(parse_txt_line(&big).is_none());
        // 10 MB value behind a valid arrow -> parsed, value length preserved.
        let line = format!("k => {}", "y".repeat(10 * 1024 * 1024));
        let r = parse_txt_line(&line).unwrap();
        assert_eq!(r.heard, "k");
        assert_eq!(r.printed.len(), 10 * 1024 * 1024);
    }

    #[test]
    fn malformed_json_import_errors_never_panics() {
        // import_replacements deserializes Vec<Replacement>; these must Err (surfaced
        // to the UI as a string), never panic. "[{}]" errors because Replacement has
        // no serde defaults; a BOM-prefixed JSON also errors (the JSON path, unlike
        // TXT, cannot eat a BOM).
        for bad in ["", "{", "[", "null", "[{}]", "not json", "[1,2]", "\u{feff}[]"] {
            let res: Result<Vec<Replacement>, _> = serde_json::from_str(bad);
            assert!(res.is_err(), "expected Err for {:?}", bad);
        }
    }

    // --- interop with the OLD (pre-escape) TXT exporter ---

    #[test]
    fn interop_old_format_literal_backslash_n_becomes_newline() {
        // A pre-escape TXT (no escaping) whose value literally held the two chars
        // `\` `n` is now unescaped into a REAL newline on import. Documented behavior
        // CHANGE, not a bug: the new exporter writes `\\n` for a literal backslash-n,
        // so new-format files are unambiguous; only legacy hand-made files shift.
        let parsed = parse_txt_line("path => a\\nb").unwrap(); // source has backslash+n
        assert_eq!(parsed.printed, "a\nb"); // -> real newline
    }
}

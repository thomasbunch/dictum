//! %APPDATA%\Dictum\config.json — atomic load/save. Missing or corrupt file
//! falls back to Config::default() and persists it (zero-config first run).

use crate::types::{app_data_dir, Config};
use std::path::{Path, PathBuf};

pub fn load() -> Config {
    load_from(&app_data_dir())
}

pub fn save(cfg: &Config) -> std::io::Result<()> {
    save_to(&app_data_dir(), cfg)
}

fn config_path(dir: &Path) -> PathBuf {
    dir.join("config.json")
}

fn load_from(dir: &Path) -> Config {
    let parsed = std::fs::read_to_string(config_path(dir))
        .ok()
        .and_then(|s| serde_json::from_str::<Config>(&s).ok());
    match parsed {
        Some(cfg) => cfg,
        None => {
            let cfg = Config::default();
            let _ = save_to(dir, &cfg); // best-effort; first run must not fail on a locked/missing dir
            cfg
        }
    }
}

fn save_to(dir: &Path, cfg: &Config) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let path = config_path(dir);
    let tmp = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(cfg).map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, &path)?; // atomic on same volume (Windows ReplaceFile-backed rename)
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Theme;

    fn temp_dir(tag: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        p.push(format!("dictum_test_config_{tag}_{nanos}"));
        p
    }

    #[test]
    fn missing_file_yields_default_and_persists() {
        let dir = temp_dir("missing");
        let cfg = load_from(&dir);
        assert_eq!(cfg, Config::default());
        assert!(config_path(&dir).exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn corrupt_file_yields_default() {
        let dir = temp_dir("corrupt");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(config_path(&dir), b"{not json").unwrap();
        let cfg = load_from(&dir);
        assert_eq!(cfg, Config::default());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn round_trip_preserves_changes() {
        let dir = temp_dir("roundtrip");
        let mut cfg = Config::default();
        cfg.theme = Theme::Obsidian;
        cfg.hotkey = "Ctrl+Alt+Space".into();
        save_to(&dir, &cfg).unwrap();
        assert_eq!(load_from(&dir), cfg);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn save_leaves_no_stray_tmp_file() {
        let dir = temp_dir("atomic");
        save_to(&dir, &Config::default()).unwrap();
        assert!(!config_path(&dir).with_extension("json.tmp").exists());
        std::fs::remove_dir_all(&dir).ok();
    }
}

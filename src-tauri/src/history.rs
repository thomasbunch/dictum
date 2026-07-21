//! rusqlite-backed transcript history at %APPDATA%\Dictum\history.db (WAL).
//! Respects retention on open and after every append; keep_transcripts=false
//! or Retention::KeepNothing makes append a no-op.

use crate::types::{app_data_dir, Config, HistoryRecord, InjectMethod, Retention, TakeMeta};
use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;

pub struct History {
    conn: Connection,
    /// Most recently deleted row, held for one undo.
    deleted: Option<HistoryRecord>,
}

impl History {
    pub fn open(cfg: &Config) -> rusqlite::Result<Self> {
        Self::open_at(&app_data_dir().join("history.db"), cfg)
    }

    pub fn open_at(path: &Path, cfg: &Config) -> rusqlite::Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).ok();
        }
        let conn = Connection::open(path)?;
        // journal_mode PRAGMA returns a row -- plain pragma_update() errors on it.
        conn.pragma_update_and_check(None, "journal_mode", "WAL", |_row| Ok(()))?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS records (
                id       INTEGER PRIMARY KEY,
                ts       INTEGER NOT NULL,
                raw      TEXT NOT NULL,
                text     TEXT NOT NULL,
                exe      TEXT,
                dur_ms   INTEGER NOT NULL DEFAULT 0,
                clipped  INTEGER NOT NULL DEFAULT 0,
                envelope TEXT,
                method   TEXT
            );",
        )?;
        // Pre-TAPE DBs lack the take-metadata columns; add them in place.
        // "duplicate column name" on an already-migrated DB is expected — ignored.
        for col in [
            "dur_ms INTEGER NOT NULL DEFAULT 0",
            "clipped INTEGER NOT NULL DEFAULT 0",
            "envelope TEXT",
            "method TEXT",
        ] {
            let _ = conn.execute(&format!("ALTER TABLE records ADD COLUMN {col}"), []);
        }
        let history = Self { conn, deleted: None };
        history.purge_retention(cfg.retention)?;
        Ok(history)
    }

    /// No-op if `!cfg.keep_transcripts` or `cfg.retention == KeepNothing`.
    pub fn append(&mut self, raw: &str, text: &str, exe: Option<&str>, meta: &TakeMeta, cfg: &Config) -> rusqlite::Result<()> {
        if !cfg.keep_transcripts || cfg.retention == Retention::KeepNothing {
            return Ok(());
        }
        self.conn.execute(
            "INSERT INTO records (ts, raw, text, exe, dur_ms, clipped, envelope, method)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                now_millis(),
                raw,
                text,
                exe,
                meta.dur_ms,
                meta.clipped,
                envelope_to_json(&meta.envelope),
                meta.method.map(method_str),
            ],
        )?;
        self.purge_retention(cfg.retention)
    }

    /// Newest-first, capped at 500. `search` LIKE-matches raw or text.
    pub fn list(&self, search: Option<&str>) -> rusqlite::Result<Vec<HistoryRecord>> {
        match search {
            Some(q) => {
                let mut stmt = self.conn.prepare(&format!(
                    "SELECT {COLS} FROM records
                     WHERE raw LIKE ?1 OR text LIKE ?1 ORDER BY ts DESC, id DESC LIMIT 500",
                ))?;
                let pat = format!("%{q}%");
                // Bind to a local so the MappedRows temporary (which borrows `stmt`)
                // is dropped at the `;`, before `stmt` — returning it as the block
                // tail keeps the borrow alive past `stmt`'s drop (E0597).
                let rows = stmt.query_map(params![pat], row_to_record)?.collect();
                rows
            }
            None => {
                let mut stmt = self.conn.prepare(&format!(
                    "SELECT {COLS} FROM records ORDER BY ts DESC, id DESC LIMIT 500",
                ))?;
                let rows = stmt.query_map([], row_to_record)?.collect();
                rows
            }
        }
    }

    /// Total line count (TAPE toolbar meta; list() is capped at 500).
    pub fn count(&self) -> i64 {
        self.conn.query_row("SELECT COUNT(*) FROM records", [], |r| r.get(0)).unwrap_or(0)
    }

    /// Deletes and buffers the row in memory (one-slot undo).
    pub fn delete(&mut self, id: i64) -> rusqlite::Result<()> {
        let row = self
            .conn
            .query_row(&format!("SELECT {COLS} FROM records WHERE id = ?1"), params![id], row_to_record)
            .optional()?;
        if let Some(rec) = row {
            self.conn.execute("DELETE FROM records WHERE id = ?1", params![id])?;
            self.deleted = Some(rec);
        }
        Ok(())
    }

    /// Re-inserts the last deleted row (id preserved). No-op if nothing buffered.
    pub fn undo_delete(&mut self) -> rusqlite::Result<()> {
        if let Some(rec) = self.deleted.take() {
            self.conn.execute(
                "INSERT INTO records (id, ts, raw, text, exe, dur_ms, clipped, envelope, method)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    rec.id,
                    rec.ts,
                    rec.raw,
                    rec.text,
                    rec.exe,
                    rec.dur_ms,
                    rec.clipped,
                    envelope_to_json(&rec.envelope),
                    rec.method,
                ],
            )?;
        }
        Ok(())
    }

    fn purge_retention(&self, retention: Retention) -> rusqlite::Result<()> {
        let ms = match retention {
            Retention::Forever => return Ok(()),
            Retention::KeepNothing => {
                self.conn.execute("DELETE FROM records", [])?;
                return Ok(());
            }
            Retention::Hours24 => 24 * 3_600_000i64,
            Retention::Days7 => 7 * 86_400_000i64,
            Retention::Days30 => 30 * 86_400_000i64,
        };
        let cutoff = now_millis() - ms;
        self.conn.execute("DELETE FROM records WHERE ts < ?1", params![cutoff])?;
        Ok(())
    }
}

const COLS: &str = "id, ts, raw, text, exe, dur_ms, clipped, envelope, method";

fn row_to_record(row: &rusqlite::Row) -> rusqlite::Result<HistoryRecord> {
    let envelope: Option<String> = row.get(7)?;
    Ok(HistoryRecord {
        id: row.get(0)?,
        ts: row.get(1)?,
        raw: row.get(2)?,
        text: row.get(3)?,
        exe: row.get(4)?,
        dur_ms: row.get(5)?,
        clipped: row.get(6)?,
        envelope: envelope
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default(),
        method: row.get(8)?,
    })
}

fn envelope_to_json(env: &[f32]) -> Option<String> {
    if env.is_empty() { None } else { serde_json::to_string(env).ok() }
}

fn method_str(m: InjectMethod) -> &'static str {
    match m {
        InjectMethod::Pasted => "pasted",
        InjectMethod::Typed => "typed",
    }
}

fn now_millis() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_db(tag: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        p.push(format!("dictum_test_history_{tag}_{nanos}"));
        p.join("history.db")
    }

    fn cleanup(path: &Path) {
        if let Some(dir) = path.parent() {
            std::fs::remove_dir_all(dir).ok();
        }
    }

    fn meta() -> TakeMeta {
        TakeMeta::default()
    }

    #[test]
    fn append_and_list_newest_first() {
        let path = temp_db("append_list");
        let cfg = Config::default();
        let mut h = History::open_at(&path, &cfg).unwrap();
        h.append("hello", "Hello.", Some("chrome.exe"), &meta(), &cfg).unwrap();
        h.append("world", "World.", None, &meta(), &cfg).unwrap();
        let records = h.list(None).unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].raw, "world"); // inserted last -> newest first
        assert_eq!(records[1].exe.as_deref(), Some("chrome.exe"));
        assert_eq!(h.count(), 2);
        cleanup(&path);
    }

    #[test]
    fn take_meta_round_trips() {
        let path = temp_db("take_meta");
        let cfg = Config::default();
        let mut h = History::open_at(&path, &cfg).unwrap();
        let m = TakeMeta {
            dur_ms: 3400,
            clipped: true,
            envelope: vec![0.1, 0.8, 0.3],
            method: Some(InjectMethod::Typed),
        };
        h.append("raw", "Text.", Some("wt.exe"), &m, &cfg).unwrap();
        let rec = &h.list(None).unwrap()[0];
        assert_eq!(rec.dur_ms, 3400);
        assert!(rec.clipped);
        assert_eq!(rec.envelope, vec![0.1, 0.8, 0.3]);
        assert_eq!(rec.method.as_deref(), Some("typed"));
        cleanup(&path);
    }

    #[test]
    fn pre_tape_db_migrates_in_place() {
        // Simulate a pre-TAPE DB (no take-metadata columns), then reopen.
        let path = temp_db("migrate");
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).unwrap();
        }
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE records (
                    id INTEGER PRIMARY KEY, ts INTEGER NOT NULL,
                    raw TEXT NOT NULL, text TEXT NOT NULL, exe TEXT
                 );
                 INSERT INTO records (ts, raw, text, exe) VALUES (1, 'old', 'Old.', NULL);",
            )
            .unwrap();
        }
        let cfg = Config { retention: Retention::Forever, ..Config::default() };
        let mut h = History::open_at(&path, &cfg).unwrap();
        let old = &h.list(None).unwrap()[0];
        assert_eq!(old.raw, "old");
        assert_eq!(old.dur_ms, 0);
        assert!(!old.clipped);
        assert!(old.envelope.is_empty());
        assert!(old.method.is_none());
        // And new appends carry the metadata.
        h.append("new", "New.", None, &TakeMeta { dur_ms: 100, ..TakeMeta::default() }, &cfg).unwrap();
        assert_eq!(h.list(None).unwrap()[0].dur_ms, 100);
        cleanup(&path);
    }

    #[test]
    fn keep_transcripts_false_is_noop() {
        let path = temp_db("noop");
        let mut cfg = Config::default();
        cfg.keep_transcripts = false;
        let mut h = History::open_at(&path, &cfg).unwrap();
        h.append("hello", "Hello.", None, &meta(), &cfg).unwrap();
        assert!(h.list(None).unwrap().is_empty());
        cleanup(&path);
    }

    #[test]
    fn keep_nothing_retention_is_noop() {
        let path = temp_db("keepnothing");
        let mut cfg = Config::default();
        cfg.retention = Retention::KeepNothing;
        let mut h = History::open_at(&path, &cfg).unwrap();
        h.append("hello", "Hello.", None, &meta(), &cfg).unwrap();
        assert!(h.list(None).unwrap().is_empty());
        cleanup(&path);
    }

    #[test]
    fn search_matches_raw_or_text() {
        let path = temp_db("search");
        let cfg = Config::default();
        let mut h = History::open_at(&path, &cfg).unwrap();
        h.append("gonna go", "Going to go.", None, &meta(), &cfg).unwrap();
        h.append("something else", "Something else.", None, &meta(), &cfg).unwrap();
        let hits = h.list(Some("gonna")).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].raw, "gonna go");
        assert_eq!(h.list(Some("Going")).unwrap().len(), 1); // matches on `text`
        cleanup(&path);
    }

    #[test]
    fn delete_and_undo_preserves_id() {
        let path = temp_db("undo");
        let cfg = Config::default();
        let mut h = History::open_at(&path, &cfg).unwrap();
        let m = TakeMeta { dur_ms: 500, clipped: true, envelope: vec![0.5], method: Some(InjectMethod::Pasted) };
        h.append("hello", "Hello.", None, &m, &cfg).unwrap();
        let id = h.list(None).unwrap()[0].id;
        h.delete(id).unwrap();
        assert!(h.list(None).unwrap().is_empty());
        h.undo_delete().unwrap();
        let records = h.list(None).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].id, id);
        // Take metadata survives the delete/undo round trip.
        assert_eq!(records[0].dur_ms, 500);
        assert_eq!(records[0].envelope, vec![0.5]);
        assert_eq!(records[0].method.as_deref(), Some("pasted"));
        cleanup(&path);
    }

    #[test]
    fn undo_without_delete_is_noop() {
        let path = temp_db("undo_noop");
        let cfg = Config::default();
        let mut h = History::open_at(&path, &cfg).unwrap();
        h.undo_delete().unwrap(); // must not error with nothing buffered
        cleanup(&path);
    }

    #[test]
    fn purge_retention_drops_old_rows() {
        let path = temp_db("retention");
        let mut cfg = Config::default();
        cfg.retention = Retention::Hours24;
        let mut h = History::open_at(&path, &cfg).unwrap();
        // insert directly with a stale timestamp (40h old), bypassing append's "now" clock
        h.conn
            .execute(
                "INSERT INTO records (ts, raw, text, exe) VALUES (?1, 'old', 'Old.', NULL)",
                params![now_millis() - 40 * 3_600_000],
            )
            .unwrap();
        h.append("fresh", "Fresh.", None, &meta(), &cfg).unwrap(); // triggers purge_retention
        let records = h.list(None).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].raw, "fresh");
        cleanup(&path);
    }

    #[test]
    fn purge_retention_keeps_inside_drops_outside_boundary() {
        let path = temp_db("retention_boundary");
        let mut cfg = Config::default();
        cfg.retention = Retention::Hours24;
        let mut h = History::open_at(&path, &cfg).unwrap();
        let window = 24 * 3_600_000i64;
        // One row just inside the 24h window, one just outside it.
        h.conn
            .execute(
                "INSERT INTO records (ts, raw, text, exe) VALUES (?1, 'inside', 'Inside.', NULL)",
                params![now_millis() - (window - 60_000)],
            )
            .unwrap();
        h.conn
            .execute(
                "INSERT INTO records (ts, raw, text, exe) VALUES (?1, 'outside', 'Outside.', NULL)",
                params![now_millis() - (window + 60_000)],
            )
            .unwrap();
        h.append("trig", "Trig.", None, &meta(), &cfg).unwrap(); // triggers purge_retention
        let raws: Vec<String> = h.list(None).unwrap().into_iter().map(|r| r.raw).collect();
        assert!(raws.contains(&"inside".to_string()), "row just inside window must be kept");
        assert!(raws.contains(&"trig".to_string()));
        assert!(!raws.contains(&"outside".to_string()), "row just outside window must be dropped");
        cleanup(&path);
    }
}

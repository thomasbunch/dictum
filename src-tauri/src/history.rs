//! rusqlite-backed transcript history at %APPDATA%\Dictum\history.db (WAL).
//! Respects retention on open and after every append; keep_transcripts=false
//! or Retention::KeepNothing makes append a no-op.

use crate::types::{app_data_dir, Config, HistoryRecord, Retention};
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
                id   INTEGER PRIMARY KEY,
                ts   INTEGER NOT NULL,
                raw  TEXT NOT NULL,
                text TEXT NOT NULL,
                exe  TEXT
            );",
        )?;
        let history = Self { conn, deleted: None };
        history.purge_retention(cfg.retention)?;
        Ok(history)
    }

    /// No-op if `!cfg.keep_transcripts` or `cfg.retention == KeepNothing`.
    pub fn append(&mut self, raw: &str, text: &str, exe: Option<&str>, cfg: &Config) -> rusqlite::Result<()> {
        if !cfg.keep_transcripts || cfg.retention == Retention::KeepNothing {
            return Ok(());
        }
        self.conn.execute(
            "INSERT INTO records (ts, raw, text, exe) VALUES (?1, ?2, ?3, ?4)",
            params![now_millis(), raw, text, exe],
        )?;
        self.purge_retention(cfg.retention)
    }

    /// Newest-first, capped at 500. `search` LIKE-matches raw or text.
    pub fn list(&self, search: Option<&str>) -> rusqlite::Result<Vec<HistoryRecord>> {
        match search {
            Some(q) => {
                let mut stmt = self.conn.prepare(
                    "SELECT id, ts, raw, text, exe FROM records
                     WHERE raw LIKE ?1 OR text LIKE ?1 ORDER BY ts DESC, id DESC LIMIT 500",
                )?;
                let pat = format!("%{q}%");
                // Bind to a local so the MappedRows temporary (which borrows `stmt`)
                // is dropped at the `;`, before `stmt` — returning it as the block
                // tail keeps the borrow alive past `stmt`'s drop (E0597).
                let rows = stmt.query_map(params![pat], row_to_record)?.collect();
                rows
            }
            None => {
                let mut stmt = self.conn.prepare(
                    "SELECT id, ts, raw, text, exe FROM records ORDER BY ts DESC, id DESC LIMIT 500",
                )?;
                let rows = stmt.query_map([], row_to_record)?.collect();
                rows
            }
        }
    }

    /// Deletes and buffers the row in memory (one-slot undo).
    pub fn delete(&mut self, id: i64) -> rusqlite::Result<()> {
        let row = self
            .conn
            .query_row("SELECT id, ts, raw, text, exe FROM records WHERE id = ?1", params![id], row_to_record)
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
                "INSERT INTO records (id, ts, raw, text, exe) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![rec.id, rec.ts, rec.raw, rec.text, rec.exe],
            )?;
        }
        Ok(())
    }

    /// e.g. "24 RECORDS · AUDIO OFF · RETENTION 7 D". AUDIO OFF is constant in v1.
    pub fn meta_line(&self, cfg: &Config) -> String {
        let count: i64 = self.conn.query_row("SELECT COUNT(*) FROM records", [], |r| r.get(0)).unwrap_or(0);
        let retention = match cfg.retention {
            Retention::KeepNothing => "KEEP NOTHING",
            Retention::Hours24 => "24 H",
            Retention::Days7 => "7 D",
            Retention::Days30 => "30 D",
            Retention::Forever => "FOREVER",
        };
        format!("{count} RECORDS · AUDIO OFF · RETENTION {retention}")
    }

    fn purge_retention(&self, retention: Retention) -> rusqlite::Result<()> {
        let ms = match retention {
            Retention::Forever => return Ok(()),
            Retention::KeepNothing => {
                self.conn.execute("DELETE FROM records", [])?;
                return Ok(());
            }
            Retention::Hours24 => 3_600_000i64,
            Retention::Days7 => 7 * 86_400_000i64,
            Retention::Days30 => 30 * 86_400_000i64,
        };
        let cutoff = now_millis() - ms;
        self.conn.execute("DELETE FROM records WHERE ts < ?1", params![cutoff])?;
        Ok(())
    }
}

fn row_to_record(row: &rusqlite::Row) -> rusqlite::Result<HistoryRecord> {
    Ok(HistoryRecord { id: row.get(0)?, ts: row.get(1)?, raw: row.get(2)?, text: row.get(3)?, exe: row.get(4)? })
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

    #[test]
    fn append_and_list_newest_first() {
        let path = temp_db("append_list");
        let cfg = Config::default();
        let mut h = History::open_at(&path, &cfg).unwrap();
        h.append("hello", "Hello.", Some("chrome.exe"), &cfg).unwrap();
        h.append("world", "World.", None, &cfg).unwrap();
        let records = h.list(None).unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].raw, "world"); // inserted last -> newest first
        assert_eq!(records[1].exe.as_deref(), Some("chrome.exe"));
        cleanup(&path);
    }

    #[test]
    fn keep_transcripts_false_is_noop() {
        let path = temp_db("noop");
        let mut cfg = Config::default();
        cfg.keep_transcripts = false;
        let mut h = History::open_at(&path, &cfg).unwrap();
        h.append("hello", "Hello.", None, &cfg).unwrap();
        assert!(h.list(None).unwrap().is_empty());
        cleanup(&path);
    }

    #[test]
    fn keep_nothing_retention_is_noop() {
        let path = temp_db("keepnothing");
        let mut cfg = Config::default();
        cfg.retention = Retention::KeepNothing;
        let mut h = History::open_at(&path, &cfg).unwrap();
        h.append("hello", "Hello.", None, &cfg).unwrap();
        assert!(h.list(None).unwrap().is_empty());
        cleanup(&path);
    }

    #[test]
    fn search_matches_raw_or_text() {
        let path = temp_db("search");
        let cfg = Config::default();
        let mut h = History::open_at(&path, &cfg).unwrap();
        h.append("gonna go", "Going to go.", None, &cfg).unwrap();
        h.append("something else", "Something else.", None, &cfg).unwrap();
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
        h.append("hello", "Hello.", None, &cfg).unwrap();
        let id = h.list(None).unwrap()[0].id;
        h.delete(id).unwrap();
        assert!(h.list(None).unwrap().is_empty());
        h.undo_delete().unwrap();
        let records = h.list(None).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].id, id);
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
        h.append("fresh", "Fresh.", None, &cfg).unwrap(); // triggers purge_retention
        let records = h.list(None).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].raw, "fresh");
        cleanup(&path);
    }

    #[test]
    fn meta_line_format() {
        let path = temp_db("meta");
        let mut cfg = Config::default();
        cfg.retention = Retention::Days7;
        let mut h = History::open_at(&path, &cfg).unwrap();
        h.append("hello", "Hello.", None, &cfg).unwrap();
        assert_eq!(h.meta_line(&cfg), "1 RECORDS · AUDIO OFF · RETENTION 7 D");
        cleanup(&path);
    }
}

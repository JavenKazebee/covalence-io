//! SQLite working store: entities, runtime journal, snapshots and assets (ADR 0006).
//!
//! Milestone 1 only needs the runtime journal and `meta` (state spec §5). The
//! store knows nothing about entry kinds: the core encodes `origin` as JSON and
//! `data` as MessagePack, and this crate keeps the bytes.

use std::path::Path;

use rusqlite::{Connection, OptionalExtension, params};

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error("database schema version {found} is newer than this build ({supported})")]
    TooNew { found: u32, supported: u32 },
    #[error("journal seq {0} is out of range for SQLite")]
    SeqRange(u64),
}

pub type Result<T, E = StoreError> = std::result::Result<T, E>;

/// One runtime journal row (state spec §2.1).
#[derive(Clone, Debug, PartialEq)]
pub struct JournalRow {
    pub seq: u64,
    pub term: u32,
    /// RFC 3339 with microseconds.
    pub ts: String,
    /// Monotonic nanoseconds since node start.
    pub mono: u64,
    /// JSON.
    pub origin: String,
    pub cause: Option<u64>,
    pub run: Option<String>,
    pub kind: String,
    /// MessagePack.
    pub data: Vec<u8>,
}

/// Each entry upgrades the schema by one version (`PRAGMA user_version`).
const MIGRATIONS: &[&str] = &[
    // 1: runtime journal and meta.
    "CREATE TABLE journal (
        seq    INTEGER PRIMARY KEY,
        term   INTEGER NOT NULL,
        ts     TEXT    NOT NULL,
        mono   INTEGER NOT NULL,
        origin TEXT    NOT NULL,
        cause  INTEGER,
        run    TEXT,
        kind   TEXT    NOT NULL,
        data   BLOB    NOT NULL
    ) STRICT;
    CREATE INDEX journal_run ON journal (run) WHERE run IS NOT NULL;
    CREATE TABLE meta (
        key   TEXT PRIMARY KEY,
        value TEXT NOT NULL
    ) STRICT;",
];

pub struct Store {
    db: Connection,
}

impl Store {
    /// Opens or creates the store at `path`, in WAL mode, and migrates it.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let db = Connection::open(path)?;
        db.pragma_update(None, "journal_mode", "WAL")?;
        db.pragma_update(None, "synchronous", "NORMAL")?;
        Self::init(db)
    }

    /// A throwaway store, for tests.
    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(mut db: Connection) -> Result<Self> {
        db.pragma_update(None, "foreign_keys", true)?;
        let current: u32 = db.pragma_query_value(None, "user_version", |row| row.get(0))?;
        let supported = MIGRATIONS.len() as u32;
        if current > supported {
            return Err(StoreError::TooNew {
                found: current,
                supported,
            });
        }
        for (i, sql) in MIGRATIONS.iter().enumerate().skip(current as usize) {
            let tx = db.transaction()?;
            tx.execute_batch(sql)?;
            tx.pragma_update(None, "user_version", i as u32 + 1)?;
            tx.commit()?;
        }
        Ok(Store { db })
    }

    pub fn append_journal(&self, row: &JournalRow) -> Result<()> {
        self.db
            .prepare_cached(
                "INSERT INTO journal (seq, term, ts, mono, origin, cause, run, kind, data)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            )?
            .execute(params![
                sql_seq(row.seq)?,
                row.term,
                row.ts,
                sql_seq(row.mono)?,
                row.origin,
                row.cause.map(sql_seq).transpose()?,
                row.run,
                row.kind,
                row.data,
            ])?;
        Ok(())
    }

    pub fn journal_entry(&self, seq: u64) -> Result<Option<JournalRow>> {
        Ok(self
            .db
            .prepare_cached(&format!("{SELECT_JOURNAL} WHERE seq = ?1"))?
            .query_row([sql_seq(seq)?], journal_row)
            .optional()?)
    }

    /// Up to `limit` entries with `seq >= from`, in order.
    pub fn journal_range(&self, from: u64, limit: usize) -> Result<Vec<JournalRow>> {
        let mut stmt = self.db.prepare_cached(&format!(
            "{SELECT_JOURNAL} WHERE seq >= ?1 ORDER BY seq LIMIT ?2"
        ))?;
        let rows = stmt.query_map(params![sql_seq(from)?, limit as i64], journal_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn last_journal_seq(&self) -> Result<Option<u64>> {
        let seq: Option<i64> = self
            .db
            .query_row("SELECT max(seq) FROM journal", [], |row| row.get(0))?;
        Ok(seq.map(|s| s as u64))
    }

    /// Deletes entries with `seq < before`, for ring-buffer retention.
    pub fn trim_journal(&self, before: u64) -> Result<usize> {
        Ok(self
            .db
            .execute("DELETE FROM journal WHERE seq < ?1", [sql_seq(before)?])?)
    }

    pub fn meta(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .db
            .query_row("SELECT value FROM meta WHERE key = ?1", [key], |row| {
                row.get(0)
            })
            .optional()?)
    }

    pub fn set_meta(&self, key: &str, value: &str) -> Result<()> {
        self.db.execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2)
             ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            [key, value],
        )?;
        Ok(())
    }
}

const SELECT_JOURNAL: &str =
    "SELECT seq, term, ts, mono, origin, cause, run, kind, data FROM journal";

fn journal_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<JournalRow> {
    Ok(JournalRow {
        seq: row.get::<_, i64>(0)? as u64,
        term: row.get(1)?,
        ts: row.get(2)?,
        mono: row.get::<_, i64>(3)? as u64,
        origin: row.get(4)?,
        cause: row.get::<_, Option<i64>>(5)?.map(|c| c as u64),
        run: row.get(6)?,
        kind: row.get(7)?,
        data: row.get(8)?,
    })
}

fn sql_seq(n: u64) -> Result<i64> {
    i64::try_from(n).map_err(|_| StoreError::SeqRange(n))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(seq: u64, cause: Option<u64>) -> JournalRow {
        JournalRow {
            seq,
            term: 1,
            ts: "2026-10-18T12:00:00.000000Z".into(),
            mono: seq * 1000,
            origin: r#"{"core":"system"}"#.into(),
            cause,
            run: None,
            kind: "log".into(),
            data: vec![0x80],
        }
    }

    #[test]
    fn journal_round_trips_and_survives_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("project.db");
        {
            let store = Store::open(&path).unwrap();
            assert_eq!(store.last_journal_seq().unwrap(), None);
            for seq in 1..=3 {
                store.append_journal(&row(seq, seq.checked_sub(1))).unwrap();
            }
            store.set_meta("term", "1").unwrap();
        }
        let store = Store::open(&path).unwrap();
        let mode: String = store
            .db
            .pragma_query_value(None, "journal_mode", |r| r.get(0))
            .unwrap();
        assert_eq!(mode, "wal");
        assert_eq!(store.last_journal_seq().unwrap(), Some(3));
        assert_eq!(store.journal_entry(2).unwrap(), Some(row(2, Some(1))));
        assert_eq!(store.journal_entry(9).unwrap(), None);
        let seqs: Vec<u64> = store
            .journal_range(2, 10)
            .unwrap()
            .iter()
            .map(|r| r.seq)
            .collect();
        assert_eq!(seqs, [2, 3]);
        assert_eq!(store.meta("term").unwrap().as_deref(), Some("1"));
    }

    #[test]
    fn rejects_duplicate_seq() {
        let store = Store::open_in_memory().unwrap();
        store.append_journal(&row(1, None)).unwrap();
        assert!(store.append_journal(&row(1, None)).is_err());
    }

    #[test]
    fn trims_old_entries() {
        let store = Store::open_in_memory().unwrap();
        for seq in 1..=5 {
            store.append_journal(&row(seq, None)).unwrap();
        }
        assert_eq!(store.trim_journal(4).unwrap(), 3);
        assert_eq!(store.journal_range(0, 10).unwrap().len(), 2);
    }
}

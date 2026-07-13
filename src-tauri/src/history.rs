//! Local time-series of usage samples, used to estimate burn velocity.
//! SQLite (bundled) at `<app_data>/history.db`.

use anyhow::{Context, Result};
use rusqlite::Connection;
use serde::Serialize;
use std::path::Path;
use std::sync::Mutex;

pub struct History {
    conn: Mutex<Connection>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Sample {
    /// epoch milliseconds
    pub ts: i64,
    pub percent: f64,
}

impl History {
    pub fn open(data_dir: &Path) -> Result<History> {
        std::fs::create_dir_all(data_dir).ok();
        let conn = Connection::open(data_dir.join("history.db"))
            .context("opening history.db")?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS samples (
                ts        INTEGER NOT NULL,
                kind      TEXT    NOT NULL,
                scope     TEXT    NOT NULL,
                percent   REAL    NOT NULL,
                resets_at INTEGER
            );
            CREATE INDEX IF NOT EXISTS idx_samples_lookup
                ON samples (kind, scope, ts);",
        )?;
        Ok(History {
            conn: Mutex::new(conn),
        })
    }

    pub fn insert(
        &self,
        ts: i64,
        kind: &str,
        scope: &str,
        percent: f64,
        resets_at: Option<i64>,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO samples (ts, kind, scope, percent, resets_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![ts, kind, scope, percent, resets_at],
        )?;
        Ok(())
    }

    /// Samples for one window since `since_ts` (epoch ms), oldest first.
    pub fn samples_since(&self, kind: &str, scope: &str, since_ts: i64) -> Result<Vec<Sample>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT ts, percent FROM samples
             WHERE kind = ?1 AND scope = ?2 AND ts >= ?3
             ORDER BY ts ASC",
        )?;
        let rows = stmt
            .query_map(rusqlite::params![kind, scope, since_ts], |r| {
                Ok(Sample {
                    ts: r.get(0)?,
                    percent: r.get(1)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Drop samples older than `before_ts` to keep the DB small.
    pub fn prune(&self, before_ts: i64) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM samples WHERE ts < ?1", rusqlite::params![before_ts])?;
        Ok(())
    }
}

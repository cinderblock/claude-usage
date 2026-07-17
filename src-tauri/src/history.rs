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

/// One completed (or in-progress) window instance, summarised. Instances are
/// keyed by `resets_at` — every sample of the same rolling window shares it.
#[derive(Debug, Clone, Serialize)]
pub struct WindowSummary {
    /// epoch ms of the window's reset (its instance key). May be null for very
    /// old rows predating `resets_at` capture — bucketed under `first_ts` then.
    pub resets_at: Option<i64>,
    /// Highest utilization the window reached (the headline of the summary view).
    pub peak_percent: f64,
    /// epoch ms of the first and last samples seen for this instance.
    pub first_ts: i64,
    pub last_ts: i64,
    /// Number of raw samples backing this instance.
    pub count: i64,
}

/// Store-wide counters used to size the DB and estimate retention trade-offs.
#[derive(Debug, Clone, Serialize)]
pub struct HistoryStats {
    pub rows: i64,
    /// epoch ms of the oldest / newest sample (null when empty).
    pub oldest_ts: Option<i64>,
    pub newest_ts: Option<i64>,
    /// On-disk size of history.db in bytes (page_count * page_size).
    pub bytes: i64,
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

    /// Peak-per-instance summaries for one window, oldest instance first.
    ///
    /// Instances can't be grouped by `resets_at` equality: the API's reported
    /// reset time jitters by a minute or so between polls of the *same* window.
    /// Instead, fold samples in time order and start a new instance whenever
    /// `resets_at` moves by more than a tolerance (real transitions jump by
    /// hours — at least the window length). Samples with a null `resets_at`
    /// are idle gaps (the API reports no reset and 0% when no window is
    /// active) and belong to no instance.
    pub fn window_summaries(&self, kind: &str, scope: &str, since_ts: i64) -> Result<Vec<WindowSummary>> {
        /// Well above observed jitter (~2 min), well below any real window hop.
        const RESET_JITTER_MS: i64 = 10 * 60 * 1000;

        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT ts, percent, resets_at FROM samples
             WHERE kind = ?1 AND scope = ?2 AND ts >= ?3
             ORDER BY ts ASC",
        )?;
        let rows = stmt
            .query_map(rusqlite::params![kind, scope, since_ts], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, f64>(1)?, r.get::<_, Option<i64>>(2)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        let mut out: Vec<WindowSummary> = Vec::new();
        let mut cur: Option<WindowSummary> = None;
        for (ts, percent, resets_at) in rows {
            let Some(ra) = resets_at else {
                // Idle: no window active. Close out any open instance so a
                // same-resets coincidence across the gap can't merge two.
                if let Some(c) = cur.take() {
                    out.push(c);
                }
                continue;
            };
            match cur.as_mut() {
                Some(c) if (ra - c.resets_at.unwrap()).abs() <= RESET_JITTER_MS => {
                    c.peak_percent = c.peak_percent.max(percent);
                    c.last_ts = ts;
                    c.count += 1;
                    c.resets_at = Some(ra); // track the freshest estimate
                }
                _ => {
                    if let Some(c) = cur.take() {
                        out.push(c);
                    }
                    cur = Some(WindowSummary {
                        resets_at: Some(ra),
                        peak_percent: percent,
                        first_ts: ts,
                        last_ts: ts,
                        count: 1,
                    });
                }
            }
        }
        if let Some(c) = cur {
            out.push(c);
        }
        Ok(out)
    }

    /// Store-wide stats for sizing/estimating retention.
    pub fn stats(&self) -> Result<HistoryStats> {
        let conn = self.conn.lock().unwrap();
        let (rows, oldest_ts, newest_ts) = conn.query_row(
            "SELECT COUNT(*), MIN(ts), MAX(ts) FROM samples",
            [],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<i64>>(1)?, r.get::<_, Option<i64>>(2)?)),
        )?;
        let page_count: i64 = conn.query_row("PRAGMA page_count", [], |r| r.get(0))?;
        let page_size: i64 = conn.query_row("PRAGMA page_size", [], |r| r.get(0))?;
        Ok(HistoryStats {
            rows,
            oldest_ts,
            newest_ts,
            bytes: page_count * page_size,
        })
    }

    /// Drop samples older than `before_ts` to keep the DB small.
    pub fn prune(&self, before_ts: i64) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM samples WHERE ts < ?1", rusqlite::params![before_ts])?;
        Ok(())
    }

    /// Keep the DB at/under `target_bytes` by dropping the oldest samples.
    /// Estimates rows-to-drop from the average bytes/row, deletes them, then
    /// VACUUMs to actually reclaim the file space (SQLite won't shrink on its
    /// own). No-op while already under budget, so the VACUUM cost is only paid
    /// when trimming.
    pub fn prune_to_size(&self, target_bytes: i64) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let page_count: i64 = conn.query_row("PRAGMA page_count", [], |r| r.get(0))?;
        let page_size: i64 = conn.query_row("PRAGMA page_size", [], |r| r.get(0))?;
        let bytes = page_count * page_size;
        if bytes <= target_bytes {
            return Ok(());
        }
        let rows: i64 = conn.query_row("SELECT COUNT(*) FROM samples", [], |r| r.get(0))?;
        if rows == 0 {
            return Ok(());
        }
        // Fraction of rows to shed to reach the target (drop a little extra so we
        // don't VACUUM on every single poll once near the cap).
        let over = (bytes - target_bytes) as f64 / bytes as f64;
        let to_drop = ((rows as f64 * over).ceil() as i64 + rows / 20).min(rows - 1).max(1);
        // Cutoff timestamp: the ts of the row just past the ones we're dropping.
        let cutoff: Option<i64> = conn
            .query_row(
                "SELECT ts FROM samples ORDER BY ts ASC LIMIT 1 OFFSET ?1",
                rusqlite::params![to_drop],
                |r| r.get(0),
            )
            .ok();
        if let Some(cutoff) = cutoff {
            conn.execute("DELETE FROM samples WHERE ts < ?1", rusqlite::params![cutoff])?;
            conn.execute_batch("VACUUM")?;
        }
        Ok(())
    }

    /// Thin samples older than `before_ts` to at most one row per hour per
    /// (kind, scope, instance), keeping the peak-percent sample in each hour so
    /// the summary/drill-in shapes survive. Recent data (>= before_ts) is left
    /// at full poll fidelity. Idempotent: re-running only removes newly-aged rows.
    /// Returns the number of rows removed.
    pub fn downsample_before(&self, before_ts: i64) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        // For each hourly bucket, keep the row with the max percent (ties → the
        // earliest rowid), delete the rest.
        let deleted = conn.execute(
            "DELETE FROM samples
             WHERE ts < ?1
               AND rowid NOT IN (
                 SELECT rowid FROM (
                   -- resets_at is hour-bucketed (not exact) because the API
                   -- jitters it by a minute or so between polls of one window.
                   SELECT rowid,
                          ROW_NUMBER() OVER (
                            PARTITION BY kind, scope, resets_at / 3600000, ts / 3600000
                            ORDER BY percent DESC, rowid ASC
                          ) AS rn
                   FROM samples
                   WHERE ts < ?1
                 )
                 WHERE rn = 1
               )",
            rusqlite::params![before_ts],
        )?;
        Ok(deleted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOUR: i64 = 3_600_000;

    fn mem() -> History {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE samples (
                ts        INTEGER NOT NULL,
                kind      TEXT    NOT NULL,
                scope     TEXT    NOT NULL,
                percent   REAL    NOT NULL,
                resets_at INTEGER
            );",
        )
        .unwrap();
        History { conn: Mutex::new(conn) }
    }

    #[test]
    fn summaries_group_by_instance_with_peaks() {
        let h = mem();
        // Two weekly instances (reset at t=100h and t=268h), 3 samples each.
        for (ts, pct, reset) in [
            (10 * HOUR, 20.0, 100 * HOUR),
            (50 * HOUR, 80.0, 100 * HOUR),
            (90 * HOUR, 60.0, 100 * HOUR), // percent can dip (API rounding); peak is 80
            (110 * HOUR, 5.0, 268 * HOUR),
            (150 * HOUR, 40.0, 268 * HOUR),
            (200 * HOUR, 95.0, 268 * HOUR),
        ] {
            h.insert(ts, "weekly_all", "all", pct, Some(reset)).unwrap();
        }
        // A different scope must not bleed in.
        h.insert(60 * HOUR, "weekly_scoped", "opus", 99.0, Some(100 * HOUR)).unwrap();

        let s = h.window_summaries("weekly_all", "all", 0).unwrap();
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].resets_at, Some(100 * HOUR));
        assert_eq!(s[0].peak_percent, 80.0);
        assert_eq!(s[0].first_ts, 10 * HOUR);
        assert_eq!(s[0].last_ts, 90 * HOUR);
        assert_eq!(s[0].count, 3);
        assert_eq!(s[1].resets_at, Some(268 * HOUR));
        assert_eq!(s[1].peak_percent, 95.0);
    }

    #[test]
    fn summaries_tolerate_resets_at_jitter() {
        let h = mem();
        // Same real window: the API wobbles the reset by ±1 min poll to poll.
        let base = 100 * HOUR;
        for (i, wobble) in [0i64, 60_000, -60_000, 60_000, 0].iter().enumerate() {
            h.insert(i as i64 * HOUR, "session", "all", 10.0 * (i as f64 + 1.0), Some(base + wobble))
                .unwrap();
        }
        let s = h.window_summaries("session", "all", 0).unwrap();
        assert_eq!(s.len(), 1, "jitter must not split an instance");
        assert_eq!(s[0].peak_percent, 50.0);
        assert_eq!(s[0].count, 5);
    }

    #[test]
    fn summaries_skip_idle_gaps_and_split_on_them() {
        let h = mem();
        // Active window → idle gap (null reset, 0%) → new window.
        h.insert(0, "session", "all", 40.0, Some(5 * HOUR)).unwrap();
        h.insert(HOUR, "session", "all", 90.0, Some(5 * HOUR)).unwrap();
        h.insert(6 * HOUR, "session", "all", 0.0, None).unwrap();
        h.insert(7 * HOUR, "session", "all", 0.0, None).unwrap();
        h.insert(8 * HOUR, "session", "all", 15.0, Some(13 * HOUR)).unwrap();

        let s = h.window_summaries("session", "all", 0).unwrap();
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].peak_percent, 90.0);
        assert_eq!(s[0].count, 2);
        assert_eq!(s[1].resets_at, Some(13 * HOUR));
        assert_eq!(s[1].count, 1);
    }

    #[test]
    fn downsample_keeps_hourly_peak_and_recent_rows() {
        let h = mem();
        // 30 samples 2 min apart inside one hour, peak (77) in the middle.
        for i in 0..30 {
            let pct = if i == 15 { 77.0 } else { 10.0 + i as f64 };
            h.insert(i * 120_000, "session", "all", pct, Some(5 * HOUR)).unwrap();
        }
        // A recent sample past the cutoff must survive untouched.
        h.insert(10 * HOUR, "session", "all", 50.0, Some(15 * HOUR)).unwrap();

        let removed = h.downsample_before(HOUR).unwrap();
        assert_eq!(removed, 29);
        let old = h.samples_since("session", "all", 0).unwrap();
        assert_eq!(old.len(), 2); // hourly peak + the recent row
        assert_eq!(old[0].percent, 77.0);
        assert_eq!(old[1].ts, 10 * HOUR);

        // Idempotent: nothing new to remove.
        assert_eq!(h.downsample_before(HOUR).unwrap(), 0);
    }

    #[test]
    fn downsample_is_per_instance_and_scope() {
        let h = mem();
        // Same hour, two scopes: each keeps its own peak.
        h.insert(0, "weekly_scoped", "opus", 10.0, Some(HOUR)).unwrap();
        h.insert(60_000, "weekly_scoped", "opus", 30.0, Some(HOUR)).unwrap();
        h.insert(0, "weekly_scoped", "sonnet", 90.0, Some(HOUR)).unwrap();
        h.insert(60_000, "weekly_scoped", "sonnet", 20.0, Some(HOUR)).unwrap();

        h.downsample_before(HOUR).unwrap();
        let opus = h.samples_since("weekly_scoped", "opus", 0).unwrap();
        let sonnet = h.samples_since("weekly_scoped", "sonnet", 0).unwrap();
        assert_eq!((opus.len(), sonnet.len()), (1, 1));
        assert_eq!(opus[0].percent, 30.0);
        assert_eq!(sonnet[0].percent, 90.0);
    }

    #[test]
    fn stats_report_span_and_rows() {
        let h = mem();
        assert_eq!(h.stats().unwrap().rows, 0);
        h.insert(5, "session", "all", 1.0, None).unwrap();
        h.insert(9, "session", "all", 2.0, None).unwrap();
        let s = h.stats().unwrap();
        assert_eq!(s.rows, 2);
        assert_eq!((s.oldest_ts, s.newest_ts), (Some(5), Some(9)));
        assert!(s.bytes > 0);
    }
}

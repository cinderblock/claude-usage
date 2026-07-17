//! Decides which sends are due right now, and persists what has already fired so
//! restarts and wake-from-sleep don't double-fire or replay stale slots.
//!
//! The core [`due`] fn is pure over the passed-in clock (any `TimeZone`), so it
//! unit-tests without touching the machine's timezone. The scheduler task in
//! `lib.rs` feeds it `Local::now()` and runs the resulting jobs.

use crate::config::Config;
use chrono::{DateTime, Datelike, NaiveTime, TimeZone};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// A due slot only fires within this window after its scheduled instant. Keeps a
/// laptop that wakes hours later from firing a long-past slot, while still
/// catching a slot missed by a minute or two of sleep/poll jitter.
const GRACE_MINS: i64 = 30;
/// The nominal 5-hour session window length, in seconds.
const FIVE_HOURS_SECS: i64 = 5 * 3600;

/// Persisted "already fired" state, separate from user-facing `Config`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ScheduleState {
    /// scheduled-message id → epoch ms of last fire.
    pub message_last_fired: HashMap<String, i64>,
    /// prime slot index (as a string) → epoch ms of last fire.
    pub prime_last_fired: HashMap<String, i64>,
}

impl ScheduleState {
    pub fn path(config_dir: &Path) -> PathBuf {
        config_dir.join("schedule_state.json")
    }

    pub fn load(config_dir: &Path) -> ScheduleState {
        match std::fs::read_to_string(Self::path(config_dir)) {
            Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
            Err(_) => ScheduleState::default(),
        }
    }

    pub fn save(&self, config_dir: &Path) -> anyhow::Result<()> {
        std::fs::create_dir_all(config_dir).ok();
        std::fs::write(Self::path(config_dir), serde_json::to_string_pretty(self)?)?;
        Ok(())
    }

    /// Record a fire for a job at `now_ms` under the right map.
    pub fn mark(&mut self, job: &SendJob, now_ms: i64) {
        if job.is_prime {
            self.prime_last_fired.insert(job.key.clone(), now_ms);
        } else {
            self.message_last_fired.insert(job.key.clone(), now_ms);
        }
    }
}

/// One thing to do this tick: send `message`, or (if `skip`) just record the
/// fire without sending because a gate said not to.
#[derive(Debug, Clone, PartialEq)]
pub struct SendJob {
    /// Provenance label for the send log: `"prime#k"` or a message id.
    pub source: String,
    pub message: String,
    pub model: String,
    /// Prime jobs get a post-send verification that the window actually started.
    pub is_prime: bool,
    /// Skip the actual send but still mark fired (a gate declined it) — so it
    /// doesn't retry every tick through the grace window.
    pub skip: bool,
    /// Key to record the fire under (message id, or prime slot index).
    pub key: String,
}

/// Evaluate all schedules against `now`. Returns jobs whose slot is currently in
/// its `[slot, slot+grace)` firing window and hasn't fired for that slot yet.
pub fn due<Tz: TimeZone>(
    now: DateTime<Tz>,
    cfg: &Config,
    state: &ScheduleState,
    session_active: bool,
) -> Vec<SendJob> {
    let now_ms = now.timestamp_millis();
    let grace_ms = GRACE_MINS * 60 * 1000;
    let mut jobs = Vec::new();

    // ---- User-defined scheduled messages ----
    let today_dow = now.weekday().num_days_from_sunday() as u8; // 0=Sun … 6=Sat
    for m in &cfg.scheduled_messages {
        if !m.enabled || m.id.is_empty() || m.message.trim().is_empty() {
            continue;
        }
        if !m.days.is_empty() && !m.days.contains(&today_dow) {
            continue;
        }
        let Some(slot_ms) = slot_ms_for(&now, &m.time_of_day) else {
            continue;
        };
        if now_ms < slot_ms || now_ms >= slot_ms + grace_ms {
            continue;
        }
        let last = state
            .message_last_fired
            .get(&m.id)
            .copied()
            .unwrap_or(i64::MIN);
        if last >= slot_ms {
            continue; // already fired for this slot
        }
        jobs.push(SendJob {
            source: m.id.clone(),
            message: m.message.clone(),
            model: m.model.clone(),
            is_prime: false,
            skip: m.only_if_session_inactive && session_active,
            key: m.id.clone(),
        });
    }

    // ---- 5-hour-window priming ----
    let p = &cfg.priming;
    if p.enabled && p.windows_per_day > 0 {
        if let Some(anchor_ms) = slot_ms_for(&now, &p.anchor_time) {
            // Step is 5h + a few seconds of slack so a prime lands just after the
            // previous window has surely reset, never on the boundary.
            let step_ms = (FIVE_HOURS_SECS + p.slot_slack_secs as i64) * 1000;
            let end_ms = p.end_of_day.as_deref().and_then(|e| slot_ms_for(&now, e));
            for k in 0..p.windows_per_day {
                let slot_ms = anchor_ms + k as i64 * step_ms;
                if let Some(end) = end_ms {
                    if slot_ms >= end {
                        break;
                    }
                }
                if now_ms < slot_ms || now_ms >= slot_ms + grace_ms {
                    continue;
                }
                let key = k.to_string();
                let last = state
                    .prime_last_fired
                    .get(&key)
                    .copied()
                    .unwrap_or(i64::MIN);
                if last >= slot_ms {
                    continue;
                }
                jobs.push(SendJob {
                    source: format!("prime#{k}"),
                    message: p.prime_prompt.clone(),
                    model: p.model.clone(),
                    is_prime: true,
                    // A window already running means there's nothing to start.
                    skip: session_active,
                    key,
                });
            }
        }
    }

    jobs
}

/// Epoch ms of today's `HH:MM` in `now`'s timezone, or `None` if unparseable.
fn slot_ms_for<Tz: TimeZone>(now: &DateTime<Tz>, hhmm: &str) -> Option<i64> {
    let t = parse_hhmm(hhmm)?;
    let naive = now.date_naive().and_time(t);
    now.timezone()
        .from_local_datetime(&naive)
        .earliest()
        .map(|dt| dt.timestamp_millis())
}

fn parse_hhmm(s: &str) -> Option<NaiveTime> {
    let (h, m) = s.split_once(':')?;
    let h: u32 = h.trim().parse().ok()?;
    let m: u32 = m.trim().parse().ok()?;
    NaiveTime::from_hms_opt(h, m, 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, PrimingConfig, ScheduledMessage};
    use chrono::{TimeZone, Utc};

    fn base_cfg() -> Config {
        // Default priming is disabled; start from Config::default and tweak.
        Config::default()
    }

    fn msg(id: &str, time: &str) -> ScheduledMessage {
        ScheduledMessage {
            id: id.into(),
            enabled: true,
            time_of_day: time.into(),
            days: vec![],
            message: "hello".into(),
            model: "haiku".into(),
            only_if_session_inactive: false,
        }
    }

    #[test]
    fn message_fires_within_grace_not_before() {
        let mut cfg = base_cfg();
        cfg.scheduled_messages = vec![msg("a", "09:00")];
        let st = ScheduleState::default();

        // 08:59 — not yet.
        let before = Utc.with_ymd_and_hms(2026, 7, 17, 8, 59, 0).unwrap();
        assert!(due(before, &cfg, &st, false).is_empty());

        // 09:05 — inside the 30-min grace.
        let at = Utc.with_ymd_and_hms(2026, 7, 17, 9, 5, 0).unwrap();
        let jobs = due(at, &cfg, &st, false);
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].key, "a");
        assert!(!jobs[0].skip);

        // 09:45 — past the grace window.
        let late = Utc.with_ymd_and_hms(2026, 7, 17, 9, 45, 0).unwrap();
        assert!(due(late, &cfg, &st, false).is_empty());
    }

    #[test]
    fn message_does_not_refire_same_slot() {
        let mut cfg = base_cfg();
        cfg.scheduled_messages = vec![msg("a", "09:00")];
        let mut st = ScheduleState::default();
        let at = Utc.with_ymd_and_hms(2026, 7, 17, 9, 5, 0).unwrap();
        let jobs = due(at, &cfg, &st, false);
        st.mark(&jobs[0], at.timestamp_millis());
        // A minute later, same slot — nothing.
        let again = Utc.with_ymd_and_hms(2026, 7, 17, 9, 6, 0).unwrap();
        assert!(due(again, &cfg, &st, false).is_empty());
    }

    #[test]
    fn weekday_filter() {
        let mut cfg = base_cfg();
        let mut m = msg("a", "09:00");
        m.days = vec![1, 2, 3, 4, 5]; // weekdays only
        cfg.scheduled_messages = vec![m];
        let st = ScheduleState::default();
        // 2026-07-18 is a Saturday.
        let sat = Utc.with_ymd_and_hms(2026, 7, 18, 9, 5, 0).unwrap();
        assert!(due(sat, &cfg, &st, false).is_empty());
        // 2026-07-17 is a Friday.
        let fri = Utc.with_ymd_and_hms(2026, 7, 17, 9, 5, 0).unwrap();
        assert_eq!(due(fri, &cfg, &st, false).len(), 1);
    }

    #[test]
    fn session_active_marks_message_skip() {
        let mut cfg = base_cfg();
        let mut m = msg("a", "09:00");
        m.only_if_session_inactive = true;
        cfg.scheduled_messages = vec![m];
        let st = ScheduleState::default();
        let at = Utc.with_ymd_and_hms(2026, 7, 17, 9, 5, 0).unwrap();
        let jobs = due(at, &cfg, &st, true);
        assert_eq!(jobs.len(), 1);
        assert!(jobs[0].skip); // present so it gets marked, but not sent
    }

    #[test]
    fn prime_slots_spaced_5h_plus_slack() {
        let mut cfg = base_cfg();
        cfg.priming = PrimingConfig {
            enabled: true,
            anchor_time: "06:00".into(),
            windows_per_day: 3,
            slot_slack_secs: 15,
            model: "haiku".into(),
            end_of_day: None,
            prime_prompt: "ok".into(),
        };
        let st = ScheduleState::default();

        // Slot 0 at 06:00.
        let s0 = Utc.with_ymd_and_hms(2026, 7, 17, 6, 0, 30).unwrap();
        let j0 = due(s0, &cfg, &st, false);
        assert_eq!(j0.len(), 1);
        assert_eq!(j0[0].source, "prime#0");

        // Slot 1 at 06:00 + 5h + 15s = 11:00:15.
        let s1 = Utc.with_ymd_and_hms(2026, 7, 17, 11, 0, 20).unwrap();
        let j1 = due(s1, &cfg, &st, false);
        assert_eq!(j1.len(), 1);
        assert_eq!(j1[0].source, "prime#1");

        // Just before slot 1 — nothing.
        let pre = Utc.with_ymd_and_hms(2026, 7, 17, 11, 0, 5).unwrap();
        assert!(due(pre, &cfg, &st, false).is_empty());
    }

    #[test]
    fn prime_skipped_when_session_active() {
        let mut cfg = base_cfg();
        cfg.priming.enabled = true;
        cfg.priming.anchor_time = "06:00".into();
        cfg.priming.windows_per_day = 1;
        let st = ScheduleState::default();
        let at = Utc.with_ymd_and_hms(2026, 7, 17, 6, 0, 30).unwrap();
        let jobs = due(at, &cfg, &st, true);
        assert_eq!(jobs.len(), 1);
        assert!(jobs[0].skip);
    }

    #[test]
    fn prime_end_of_day_drops_late_slots() {
        let mut cfg = base_cfg();
        cfg.priming = PrimingConfig {
            enabled: true,
            anchor_time: "06:00".into(),
            windows_per_day: 4,
            slot_slack_secs: 15,
            model: "haiku".into(),
            end_of_day: Some("16:00".into()), // slots 0,1,2 at 06:00/11:00/16:00 — 16:00 dropped
            prime_prompt: "ok".into(),
        };
        let st = ScheduleState::default();
        // Slot 2 nominal = 06:00 + 2*(5h+15s) = 16:00:30 — at/after end_of_day, dropped.
        let s2 = Utc.with_ymd_and_hms(2026, 7, 17, 16, 0, 40).unwrap();
        assert!(due(s2, &cfg, &st, false).is_empty());
    }
}

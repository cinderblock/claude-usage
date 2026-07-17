//! User-tunable settings, persisted as TOML-ish JSON in the app config dir.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// How often to poll the usage endpoint.
    pub poll_interval_secs: u64,
    /// Alert only when projected to hit the cap at least this many minutes
    /// before the window resets. A small gap (you cap ~5 min before reset) is
    /// noise; the default (~1 hour) only fires on a genuine overshoot.
    pub projection_margin_mins: i64,
    /// Trailing span used to estimate current burn velocity.
    pub velocity_window_hours: f64,
    /// Suppress projection warnings until at least this fraction of the window
    /// has elapsed (early-window velocity is too noisy to trust)...
    pub min_elapsed_frac: f64,
    /// ...unless absolute usage is already this high, i.e. you're well beyond
    /// where you should be regardless of how little time has passed.
    pub well_beyond_pct: f64,
    /// Secondary nudge: warn if already at/above this % and still climbing.
    pub near_cap_pct: f64,
    /// Only alert when the probability of capping early (from the velocity
    /// fit's uncertainty) is at least this (0–1). Judges the area under the
    /// rate distribution rather than just the mean projection.
    pub cap_confidence: f64,
    /// An alert condition must hold continuously this long before it notifies,
    /// and be clear this long before it releases. Debounces noisy fits that
    /// flap across the threshold poll-to-poll.
    pub alert_sustain_mins: i64,
    /// Also treat the API's own `severity` field as an alert trigger.
    pub use_api_severity: bool,
    /// Refresh + write back the token when expired (keeps Claude Code in sync).
    pub self_refresh_tokens: bool,
    /// Master switch for OS notifications.
    pub notifications_enabled: bool,
    /// How the long-term history store is bounded: keep everything
    /// (`"unlimited"`), cap by age (`"time"`), or cap by on-disk size (`"size"`).
    pub history_retention_mode: RetentionMode,
    /// Age cap in days, when `history_retention_mode == Time`.
    pub history_retention_days: u32,
    /// On-disk cap in megabytes, when `history_retention_mode == Size`.
    pub history_retention_mb: u32,
    /// When on, samples older than `history_downsample_after_days` are thinned to
    /// one (peak-preserving) point per hour per window instance. Off by default —
    /// full fidelity is kept for every retained sample.
    pub history_downsample: bool,
    /// Only downsample samples older than this many days (recent history stays at
    /// full poll fidelity so the live charts are unaffected).
    pub history_downsample_after_days: u32,

    // ---- Sending (scheduled messages + window priming) ----
    /// Path to the Claude Code CLI used for sending. Empty = autodetect
    /// (`claude` on PATH, then `~/.local/bin/claude*`).
    pub claude_binary_path: String,
    /// User-defined scheduled prompts.
    pub scheduled_messages: Vec<ScheduledMessage>,
    /// 5-hour-window priming settings.
    pub priming: PrimingConfig,
}

/// Default `claude --model` value for new sends — the cheapest model, since a
/// prime only needs to *start* the shared 5h window, not do real work.
fn default_model() -> String {
    "haiku".to_string()
}

/// One user-defined scheduled message.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ScheduledMessage {
    /// Stable id, generated when the row is created in the UI. Keys the
    /// persisted "last fired" state so edits/reorders don't double-fire.
    pub id: String,
    pub enabled: bool,
    /// Local wall-clock time, "HH:MM".
    pub time_of_day: String,
    /// Weekdays this fires on: 0=Sun … 6=Sat. Empty = every day.
    pub days: Vec<u8>,
    /// The prompt sent to `claude -p`.
    pub message: String,
    /// `claude --model` value (alias like "haiku" or a full model id).
    pub model: String,
    /// Skip the send when a 5h session window is already active.
    pub only_if_session_inactive: bool,
}

impl Default for ScheduledMessage {
    fn default() -> Self {
        Self {
            id: String::new(),
            enabled: true,
            time_of_day: "09:00".into(),
            days: Vec::new(),
            message: String::new(),
            model: default_model(),
            only_if_session_inactive: false,
        }
    }
}

/// 5-hour-window priming: send a tiny message at anchor + k·(5h + slack) so a
/// fresh session window starts early, letting the day hold 3 windows instead of 2.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PrimingConfig {
    pub enabled: bool,
    /// Local "HH:MM" the first 5h window of the day should start.
    pub anchor_time: String,
    /// How many windows to prime per day (slots at anchor + k·step, k in 0..N).
    pub windows_per_day: u8,
    /// Seconds of slack added to each 5h step so a prime lands just *after* the
    /// previous window has surely reset, never exactly on the boundary where
    /// timing jitter could drop it into the old window (wasted) or race the
    /// reset. Small on purpose — a few seconds is enough.
    pub slot_slack_secs: u32,
    /// `claude --model` for prime messages.
    pub model: String,
    /// Optional local "HH:MM"; slots at/after this are dropped.
    pub end_of_day: Option<String>,
    /// The tiny prompt used to prime. Kept text-only so a headless run never
    /// triggers a tool/permission prompt that could hang.
    pub prime_prompt: String,
}

impl Default for PrimingConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            anchor_time: "06:00".into(),
            windows_per_day: 3,
            slot_slack_secs: 15,
            model: default_model(),
            end_of_day: None,
            prime_prompt: "Reply with just: ok".into(),
        }
    }
}

/// How the history store decides what to keep.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RetentionMode {
    /// Never prune — keep every sample forever.
    Unlimited,
    /// Prune samples older than `history_retention_days`.
    Time,
    /// Prune oldest samples to keep the DB under `history_retention_mb`.
    Size,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            poll_interval_secs: 120,
            projection_margin_mins: 60,
            velocity_window_hours: 6.0,
            min_elapsed_frac: 0.15,
            well_beyond_pct: 60.0,
            near_cap_pct: 95.0,
            cap_confidence: 0.75,
            alert_sustain_mins: 10,
            use_api_severity: true,
            self_refresh_tokens: true,
            notifications_enabled: true,
            history_retention_mode: RetentionMode::Unlimited,
            history_retention_days: 365,
            history_retention_mb: 100,
            history_downsample: false,
            history_downsample_after_days: 60,
            claude_binary_path: String::new(),
            scheduled_messages: Vec::new(),
            priming: PrimingConfig::default(),
        }
    }
}

impl Config {
    pub fn path(config_dir: &Path) -> PathBuf {
        config_dir.join("config.json")
    }

    pub fn load(config_dir: &Path) -> Config {
        let path = Self::path(config_dir);
        match std::fs::read_to_string(&path) {
            Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
            Err(_) => Config::default(),
        }
    }

    pub fn save(&self, config_dir: &Path) -> Result<()> {
        std::fs::create_dir_all(config_dir).ok();
        let path = Self::path(config_dir);
        std::fs::write(&path, serde_json::to_string_pretty(self)?)
            .with_context(|| format!("writing {}", path.display()))?;
        Ok(())
    }
}

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

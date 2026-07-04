//! Turns projections into de-duplicated alerts. An alert fires once when its
//! condition first becomes true, and re-arms only when the window resets (its
//! `resets_at` advances) or the condition clears — never once per poll.

use crate::config::Config;
use crate::metrics::{pretty_kind, Projection};
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct Alert {
    pub key: String,
    pub title: String,
    pub body: String,
}

/// Remembers which (window, rule) pairs are currently firing, and for which
/// window instance (keyed by resets_at epoch-ms), so we can re-arm on reset.
#[derive(Default)]
pub struct AlertState {
    active: HashMap<String, i64>,
}

fn resets_key(p: &Projection) -> i64 {
    p.resets_at.map(|r| r.timestamp_millis()).unwrap_or(0)
}

fn severity_rank(sev: &Option<String>) -> u8 {
    match sev.as_deref() {
        Some("critical") | Some("exceeded") => 3,
        Some("warning") => 2,
        Some("normal") | None => 0,
        Some(_) => 1,
    }
}

impl AlertState {
    /// Evaluate all projections and return the alerts that should be raised now.
    pub fn evaluate(&mut self, projections: &[Projection], cfg: &Config) -> Vec<Alert> {
        let mut out = Vec::new();
        // Track which keys are candidates this round so we can clear stale ones.
        let mut seen: HashMap<String, i64> = HashMap::new();

        for p in projections {
            let name = p.scope_label.clone().unwrap_or_else(|| pretty_kind(&p.kind));
            let rk = resets_key(p);

            // Rule 1 (primary): projected to hit the wall before reset, and past
            // the noisy early-window phase (or already well beyond).
            if p.alert_worthy {
                let key = format!("proj:{}:{}", p.kind, p.scope_key);
                seen.insert(key.clone(), rk);
                if self.should_fire(&key, rk) {
                    out.push(Alert {
                        key: key.clone(),
                        title: format!("⚠ {name} on track to run out"),
                        body: p.summary.clone(),
                    });
                }
            }

            // Rule 2 (secondary): already near the cap and still climbing.
            let climbing = p.rate_per_hour.map(|r| r > 0.01).unwrap_or(false);
            if p.percent >= cfg.near_cap_pct && climbing {
                let key = format!("near:{}:{}", p.kind, p.scope_key);
                seen.insert(key.clone(), rk);
                if self.should_fire(&key, rk) {
                    out.push(Alert {
                        key: key.clone(),
                        title: format!("{name} nearly maxed"),
                        body: format!("{:.0}% used and still climbing", p.percent),
                    });
                }
            }

            // Rule 3 (optional): the API's own severity says warning+.
            if cfg.use_api_severity && severity_rank(&p.severity) >= 2 {
                let key = format!("sev:{}:{}", p.kind, p.scope_key);
                seen.insert(key.clone(), rk);
                if self.should_fire(&key, rk) {
                    out.push(Alert {
                        key: key.clone(),
                        title: format!("{name}: {} from Claude", p.severity.clone().unwrap_or_default()),
                        body: format!("{:.0}% used", p.percent),
                    });
                }
            }
        }

        // Retain only keys still active this round (so a cleared condition can
        // fire again later), and update their window instance markers.
        self.active = seen;
        out
    }

    /// Fire if this key isn't already active for this window instance.
    fn should_fire(&self, key: &str, resets_at_ms: i64) -> bool {
        match self.active.get(key) {
            Some(prev) => *prev != resets_at_ms, // new window instance → re-arm
            None => true,
        }
    }
}

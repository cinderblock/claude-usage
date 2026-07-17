//! Turns projections into debounced, latched alerts.
//!
//! Noisy velocity fits make the raw conditions flap poll-to-poll, and a naive
//! fire-once-until-clear scheme re-arms on every flap — constant toasts. So
//! each rule is a latch: the condition must hold *continuously* for the
//! sustain period before it engages (fires a notification), and it releases
//! only after being clear that long. Once fired for a window instance it
//! re-fires only on a real escalation (its level climbing a full step past
//! where it last fired), never on flapping. A window reset re-arms everything.

use crate::config::Config;
use crate::metrics::{pretty_kind, Projection};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone)]
pub struct Alert {
    pub key: String,
    pub title: String,
    pub body: String,
}

/// Re-fire thresholds: how far a rule's level must climb past the level it
/// last fired at to justify another notification in the same window instance.
const PROJ_ESCALATION: f64 = 0.15; // cap probability (0–1)
const NEAR_ESCALATION: f64 = 5.0; // percent used
const SEV_ESCALATION: f64 = 1.0; // severity rank

#[derive(Debug, Clone, Default)]
struct RuleState {
    /// Window instance this state belongs to (resets_at epoch-ms).
    window_instance: i64,
    /// When the condition became continuously true / false.
    true_since: Option<i64>,
    false_since: Option<i64>,
    /// Latched alert state: sustained-true engages, sustained-false releases.
    engaged: bool,
    /// Level at the last notification for this window instance.
    fired_level: Option<f64>,
}

#[derive(Default)]
pub struct AlertState {
    rules: HashMap<String, RuleState>,
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
    pub fn evaluate(
        &mut self,
        projections: &[Projection],
        cfg: &Config,
        now_ms: i64,
    ) -> Vec<Alert> {
        let sustain_ms = cfg.alert_sustain_mins.max(0) * 60_000;
        let mut out = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();

        for p in projections {
            let name = p
                .scope_label
                .clone()
                .unwrap_or_else(|| pretty_kind(&p.kind));
            let rk = resets_key(p);

            // Rule 1 (primary): sustained projection of hitting the wall.
            // `alert_worthy` already folds in the early-window gate and the
            // cap-probability confidence bar.
            let key = format!("proj:{}:{}", p.kind, p.scope_key);
            seen.insert(key.clone());
            let st = self.rules.entry(key.clone()).or_default();
            let level = p.cap_probability.unwrap_or(1.0);
            if Self::step(
                st,
                &key,
                rk,
                p.alert_worthy,
                level,
                PROJ_ESCALATION,
                now_ms,
                sustain_ms,
            ) {
                out.push(Alert {
                    key,
                    title: format!("⚠ {name} on track to run out"),
                    body: p.summary.clone(),
                });
            }

            // Rule 2 (secondary): already near the cap and still climbing.
            let climbing = p.rate_per_hour.map(|r| r > 0.01).unwrap_or(false);
            let key = format!("near:{}:{}", p.kind, p.scope_key);
            seen.insert(key.clone());
            let st = self.rules.entry(key.clone()).or_default();
            let cond = p.percent >= cfg.near_cap_pct && climbing;
            if Self::step(
                st,
                &key,
                rk,
                cond,
                p.percent,
                NEAR_ESCALATION,
                now_ms,
                sustain_ms,
            ) {
                out.push(Alert {
                    key,
                    title: format!("{name} nearly maxed"),
                    body: format!("{:.0}% used and still climbing", p.percent),
                });
            }

            // Rule 3 (optional): the API's own severity says warning+.
            let rank = severity_rank(&p.severity);
            let key = format!("sev:{}:{}", p.kind, p.scope_key);
            seen.insert(key.clone());
            let st = self.rules.entry(key.clone()).or_default();
            let cond = cfg.use_api_severity && rank >= 2;
            if Self::step(
                st,
                &key,
                rk,
                cond,
                rank as f64,
                SEV_ESCALATION,
                now_ms,
                sustain_ms,
            ) {
                out.push(Alert {
                    key,
                    title: format!(
                        "{name}: {} from Claude",
                        p.severity.clone().unwrap_or_default()
                    ),
                    body: format!("{:.0}% used", p.percent),
                });
            }
        }

        // Drop state for windows that vanished from the response.
        self.rules.retain(|k, _| seen.contains(k));
        out
    }

    /// Whether the primary (projected-overrun) rule is latched for a window.
    /// Drives the stable red tray/UI state.
    pub fn proj_engaged(&self, kind: &str, scope_key: &str) -> bool {
        self.rules
            .get(&format!("proj:{kind}:{scope_key}"))
            .map_or(false, |s| s.engaged)
    }

    /// Advance one rule's latch; returns true when a notification should fire.
    fn step(
        st: &mut RuleState,
        key: &str,
        window_instance: i64,
        condition: bool,
        level: f64,
        escalation: f64,
        now_ms: i64,
        sustain_ms: i64,
    ) -> bool {
        if st.window_instance != window_instance {
            if st.engaged {
                log::debug!("latch {key}: released (window reset)");
            }
            *st = RuleState {
                window_instance,
                ..RuleState::default()
            };
        }

        if !condition {
            st.true_since = None;
            if st.engaged {
                let since = *st.false_since.get_or_insert(now_ms);
                if now_ms - since >= sustain_ms {
                    st.engaged = false;
                    st.false_since = None;
                    log::info!("latch {key}: released (clear for sustain period)");
                }
            }
            return false;
        }

        st.false_since = None;
        let since = *st.true_since.get_or_insert(now_ms);
        if !st.engaged && now_ms - since >= sustain_ms {
            st.engaged = true;
            log::info!("latch {key}: engaged (level {level:.2})");
        }
        if !st.engaged {
            return false;
        }
        let fire = match st.fired_level {
            None => true,
            Some(prev) => level >= prev + escalation,
        };
        if fire {
            st.fired_level = Some(level);
        }
        fire
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, Utc};

    const MIN: i64 = 60_000;

    fn cfg() -> Config {
        Config {
            alert_sustain_mins: 5,
            ..Config::default()
        }
    }

    fn proj(alert_worthy: bool, cap_probability: f64, resets_at: &str) -> Projection {
        Projection {
            kind: "session".into(),
            scope_key: "all".into(),
            scope_label: None,
            percent: 50.0,
            severity: None,
            resets_at: Some(
                DateTime::parse_from_rfc3339(resets_at)
                    .unwrap()
                    .with_timezone(&Utc),
            ),
            window_len_hours: 5.0,
            time_to_reset_hours: 2.0,
            elapsed_frac: 0.6,
            rate_per_hour: Some(20.0),
            rate_stderr: Some(2.0),
            eta_to_100_hours: Some(1.0),
            cap_eta: None,
            projected_final_pct: 110.0,
            projected_final_low_pct: Some(95.0),
            projected_final_high_pct: Some(125.0),
            cap_probability: Some(cap_probability),
            will_hit_wall: alert_worthy,
            alert_worthy,
            alert_engaged: false,
            dollars: None,
            summary: "test".into(),
        }
    }

    const RESET: &str = "2026-07-07T12:00:00Z";

    #[test]
    fn flapping_condition_fires_once() {
        let mut st = AlertState::default();
        let c = cfg();
        let mut fired = 0;
        // Condition flaps every poll around the threshold for an hour.
        for i in 0..60 {
            let worthy = i % 3 != 2; // true, true, false, true, true, false…
            fired += st.evaluate(&[proj(worthy, 0.8, RESET)], &c, i * MIN).len();
        }
        // Flapping faster than the sustain period never engages at all — the
        // latch demands 5 continuous minutes of truth first.
        assert_eq!(fired, 0, "sub-sustain flapping must not notify");

        // Now hold it true: engages once, fires once, then stays quiet.
        for i in 60..90 {
            fired += st.evaluate(&[proj(true, 0.8, RESET)], &c, i * MIN).len();
        }
        assert_eq!(fired, 1, "sustained condition fires exactly once");

        // Brief dropouts after firing don't re-arm it.
        for i in 90..150 {
            let worthy = i % 4 != 3;
            fired += st.evaluate(&[proj(worthy, 0.8, RESET)], &c, i * MIN).len();
        }
        assert_eq!(fired, 1, "post-fire flapping must not re-notify");
    }

    #[test]
    fn escalation_refires() {
        let mut st = AlertState::default();
        let c = cfg();
        let mut fired = 0;
        for i in 0..10 {
            fired += st.evaluate(&[proj(true, 0.76, RESET)], &c, i * MIN).len();
        }
        assert_eq!(fired, 1, "fires at initial odds");
        // Odds creep up a little: no re-fire below the escalation step.
        for i in 10..20 {
            fired += st.evaluate(&[proj(true, 0.85, RESET)], &c, i * MIN).len();
        }
        assert_eq!(fired, 1, "sub-step creep stays quiet");
        // A full step up re-fires once.
        for i in 20..30 {
            fired += st.evaluate(&[proj(true, 0.95, RESET)], &c, i * MIN).len();
        }
        assert_eq!(fired, 2, "escalation past the step re-fires once");
    }

    #[test]
    fn window_reset_rearms() {
        let mut st = AlertState::default();
        let c = cfg();
        let mut fired = 0;
        for i in 0..10 {
            fired += st.evaluate(&[proj(true, 0.9, RESET)], &c, i * MIN).len();
        }
        assert_eq!(fired, 1);
        // Same condition, new window instance → sustain again, fire again.
        for i in 10..20 {
            fired += st
                .evaluate(&[proj(true, 0.9, "2026-07-07T17:00:00Z")], &c, i * MIN)
                .len();
        }
        assert_eq!(fired, 2, "new window re-arms the latch");
    }

    #[test]
    fn engagement_latches_and_releases() {
        let mut st = AlertState::default();
        let c = cfg();
        for i in 0..10 {
            st.evaluate(&[proj(true, 0.9, RESET)], &c, i * MIN);
        }
        assert!(st.proj_engaged("session", "all"), "sustained-true engages");
        // One clear poll: still engaged (release also needs sustain).
        st.evaluate(&[proj(false, 0.2, RESET)], &c, 10 * MIN);
        assert!(
            st.proj_engaged("session", "all"),
            "brief clear keeps the latch"
        );
        // Sustained clear releases.
        for i in 11..20 {
            st.evaluate(&[proj(false, 0.2, RESET)], &c, i * MIN);
        }
        assert!(
            !st.proj_engaged("session", "all"),
            "sustained clear releases"
        );
    }
}

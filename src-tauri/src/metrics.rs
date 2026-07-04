//! Pace / burn-rate projection — the core signal.
//!
//! A raw utilization threshold is deliberately NOT the alarm. Being at 80% when
//! you're 90% through a window means you're under pace and will coast to reset.
//! What matters: your recent velocity, and whether it hits the cap before the
//! window resets. See metrics::project.

use crate::history::Sample;
use chrono::{DateTime, Duration, Utc};
use serde::Serialize;

/// Length of a window, derived from its `kind`.
pub fn window_len_hours(kind: &str) -> f64 {
    if kind.starts_with("weekly") {
        7.0 * 24.0
    } else {
        // "session" / five-hour
        5.0
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Projection {
    pub kind: String,
    pub scope_key: String,
    pub scope_label: Option<String>,
    pub percent: f64,
    pub severity: Option<String>,
    pub resets_at: Option<DateTime<Utc>>,
    pub window_len_hours: f64,
    pub time_to_reset_hours: f64,
    pub elapsed_frac: f64,
    /// %/hour, from a least-squares fit over the recent velocity window.
    /// None when there isn't enough history yet.
    pub rate_per_hour: Option<f64>,
    /// Hours until predicted to reach 100% (None if flat/declining or no rate).
    pub eta_to_100_hours: Option<f64>,
    /// When we predict hitting 100% (None if not projected to).
    pub cap_eta: Option<DateTime<Utc>>,
    /// Best estimate of utilization at reset time.
    pub projected_final_pct: f64,
    /// True when projected to hit the cap meaningfully before reset. This is the
    /// raw projection — always computed, shown in the UI even early in a window.
    pub will_hit_wall: bool,
    /// True when `will_hit_wall` AND it's worth actively alerting: either we're
    /// past the noisy early phase of the window, or usage is already well beyond
    /// where it should be. Drives notifications + the red tray state.
    pub alert_worthy: bool,
    /// Human one-liner for UI/notifications.
    pub summary: String,
}

/// Inputs describing one window's current state.
pub struct WindowState<'a> {
    pub kind: &'a str,
    pub scope_key: &'a str,
    pub scope_label: Option<String>,
    pub percent: f64,
    pub severity: Option<String>,
    pub resets_at: Option<DateTime<Utc>>,
}

/// Least-squares slope of percent-vs-hours. Returns %/hour, or None if fewer
/// than 2 distinct-time points or a degenerate time span.
fn fit_rate(samples: &[Sample]) -> Option<f64> {
    if samples.len() < 2 {
        return None;
    }
    let t0 = samples[0].ts as f64;
    // x in hours since first sample, y in percent.
    let xs: Vec<f64> = samples.iter().map(|s| (s.ts as f64 - t0) / 3_600_000.0).collect();
    let ys: Vec<f64> = samples.iter().map(|s| s.percent).collect();
    let n = xs.len() as f64;
    let sx: f64 = xs.iter().sum();
    let sy: f64 = ys.iter().sum();
    let sxx: f64 = xs.iter().map(|x| x * x).sum();
    let sxy: f64 = xs.iter().zip(&ys).map(|(x, y)| x * y).sum();
    let denom = n * sxx - sx * sx;
    if denom.abs() < 1e-9 {
        return None;
    }
    Some((n * sxy - sx * sy) / denom)
}

/// Compute the projection for one window given its recent samples (already
/// filtered to the current window instance, oldest first) and config knobs.
pub struct ProjectOpts {
    pub margin_mins: i64,
    /// Below this fraction elapsed, projection warnings are suppressed as noise…
    pub min_elapsed_frac: f64,
    /// …unless current utilization is already at/above this.
    pub well_beyond_pct: f64,
}

pub fn project(
    w: &WindowState,
    samples: &[Sample],
    now: DateTime<Utc>,
    opts: &ProjectOpts,
) -> Projection {
    let window_len_hours = window_len_hours(w.kind);

    let (time_to_reset_hours, cap_eta_base) = match w.resets_at {
        Some(r) => (
            (r - now).num_seconds() as f64 / 3600.0,
            Some(r),
        ),
        None => (window_len_hours, None),
    };
    let _ = cap_eta_base;

    let window_start = w.resets_at.map(|r| r - Duration::hours(window_len_hours as i64));
    let elapsed_frac = match window_start {
        Some(start) => {
            let elapsed = (now - start).num_seconds() as f64 / 3600.0;
            (elapsed / window_len_hours).clamp(0.0, 1.0)
        }
        None => 0.0,
    };

    let rate_per_hour = fit_rate(samples);

    // ETA + projected-final. Prefer measured velocity; fall back to even-pace
    // extrapolation from window start when we lack a usable rate.
    let mut eta_to_100_hours = None;
    let mut cap_eta = None;
    let projected_final_pct;

    let usable_rate = rate_per_hour.filter(|r| *r > 0.01);
    if let Some(rate) = usable_rate {
        let eta = (100.0 - w.percent) / rate;
        if eta.is_finite() && eta >= 0.0 {
            eta_to_100_hours = Some(eta);
            cap_eta = Some(now + Duration::seconds((eta * 3600.0) as i64));
        }
        projected_final_pct = w.percent + rate * time_to_reset_hours;
    } else if elapsed_frac > 0.02 {
        // even-pace fallback
        let final_est = w.percent / elapsed_frac;
        projected_final_pct = final_est;
        if final_est > 100.0 {
            // estimate when the even-pace line crosses 100
            let rate_even = w.percent / (elapsed_frac * window_len_hours); // %/hr since start
            if rate_even > 0.0 {
                let eta = (100.0 - w.percent) / rate_even;
                eta_to_100_hours = Some(eta);
                cap_eta = Some(now + Duration::seconds((eta * 3600.0) as i64));
            }
        }
    } else {
        projected_final_pct = w.percent;
    }

    // "Hits the wall": projected to reach 100% at least `margin` before reset.
    // This is the raw projection — computed and shown even early in a window.
    let margin_hours = opts.margin_mins as f64 / 60.0;
    let will_hit_wall = match eta_to_100_hours {
        Some(eta) => eta < (time_to_reset_hours - margin_hours),
        None => false,
    };

    // Alert-worthiness gates the noise: early in a window the velocity fit is
    // unreliable (a small initial burst extrapolates to "will run out"), so we
    // don't actively warn unless we're past the early phase OR usage is already
    // well beyond where it should be.
    let too_early = elapsed_frac < opts.min_elapsed_frac && w.percent < opts.well_beyond_pct;
    let alert_worthy = will_hit_wall && !too_early;

    let summary = build_summary(w, will_hit_wall, cap_eta, time_to_reset_hours, projected_final_pct, rate_per_hour);

    Projection {
        kind: w.kind.to_string(),
        scope_key: w.scope_key.to_string(),
        scope_label: w.scope_label.clone(),
        percent: w.percent,
        severity: w.severity.clone(),
        resets_at: w.resets_at,
        window_len_hours,
        time_to_reset_hours,
        elapsed_frac,
        rate_per_hour,
        eta_to_100_hours,
        cap_eta,
        projected_final_pct,
        will_hit_wall,
        alert_worthy,
        summary,
    }
}

fn fmt_dur(hours: f64) -> String {
    if hours < 0.0 {
        return "now".into();
    }
    let total_min = (hours * 60.0).round() as i64;
    let d = total_min / (60 * 24);
    let h = (total_min % (60 * 24)) / 60;
    let m = total_min % 60;
    if d > 0 {
        format!("{d}d {h}h")
    } else if h > 0 {
        format!("{h}h {m}m")
    } else {
        format!("{m}m")
    }
}

fn build_summary(
    w: &WindowState,
    will_hit_wall: bool,
    cap_eta: Option<DateTime<Utc>>,
    time_to_reset_hours: f64,
    projected_final_pct: f64,
    rate: Option<f64>,
) -> String {
    let name = w.scope_label.clone().unwrap_or_else(|| pretty_kind(w.kind));
    if will_hit_wall {
        if let (Some(cap), Some(_)) = (cap_eta, rate) {
            let lead = time_to_reset_hours - (cap - Utc::now()).num_seconds() as f64 / 3600.0;
            let _ = lead;
            let to_cap = (cap - Utc::now()).num_seconds() as f64 / 3600.0;
            return format!(
                "{name}: at this pace you cap in {} (window resets in {})",
                fmt_dur(to_cap),
                fmt_dur(time_to_reset_hours)
            );
        }
        return format!("{name}: projected to hit the cap before reset");
    }
    format!(
        "{name}: {:.0}% now, ~{:.0}% projected by reset ({} left)",
        w.percent,
        projected_final_pct.min(999.0),
        fmt_dur(time_to_reset_hours)
    )
}

pub fn pretty_kind(kind: &str) -> String {
    match kind {
        "session" => "5-hour".into(),
        "weekly_all" => "Weekly".into(),
        "weekly_scoped" => "Weekly (model)".into(),
        other => other.replace('_', " "),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(min_ago_from: i64, base_ts: i64, pct: f64) -> Sample {
        Sample {
            ts: base_ts + min_ago_from * 60_000,
            percent: pct,
        }
    }

    fn opts() -> ProjectOpts {
        ProjectOpts {
            margin_mins: 30,
            min_elapsed_frac: 0.15,
            well_beyond_pct: 60.0,
        }
    }

    #[test]
    fn even_pace_does_not_hit_wall() {
        // 90% of the way through a 7-day window at 80% used → under pace.
        let now = DateTime::parse_from_rfc3339("2026-07-03T00:00:00Z").unwrap().with_timezone(&Utc);
        let resets_at = now + Duration::hours(16); // ~90% through 7d window
        let base = (now - Duration::hours(6)).timestamp_millis();
        // Flat-ish recent samples: ~1%/hr
        let samples = vec![
            sample(0, base, 74.0),
            sample(180, base, 77.0),
            sample(360, base, 80.0),
        ];
        let w = WindowState {
            kind: "weekly_all",
            scope_key: "all",
            scope_label: None,
            percent: 80.0,
            severity: None,
            resets_at: Some(resets_at),
        };
        let p = project(&w, &samples, now, &opts());
        assert!(!p.will_hit_wall, "should not warn: {}", p.summary);
        assert!(!p.alert_worthy);
    }

    #[test]
    fn early_window_burst_projects_but_does_not_alert() {
        // ~1 day into a 7-day window (14% elapsed), burning fast at 40%.
        // The projection sees a wall, but it's too early + not well beyond, so
        // we compute/show it without pushing an alert.
        let now = DateTime::parse_from_rfc3339("2026-07-03T00:00:00Z").unwrap().with_timezone(&Utc);
        let resets_at = now + Duration::hours(6 * 24);
        let base = (now - Duration::hours(6)).timestamp_millis();
        let samples = vec![
            sample(0, base, 10.0),
            sample(180, base, 25.0),
            sample(360, base, 40.0),
        ];
        let w = WindowState {
            kind: "weekly_all",
            scope_key: "all",
            scope_label: None,
            percent: 40.0,
            severity: None,
            resets_at: Some(resets_at),
        };
        let p = project(&w, &samples, now, &opts());
        assert!(p.will_hit_wall, "projection should see the wall: {}", p.summary);
        assert!(!p.alert_worthy, "should not alert this early at 40%");
    }

    #[test]
    fn early_window_but_well_beyond_alerts() {
        // Same early phase, but already at 70% (> well_beyond 60) → alert.
        let now = DateTime::parse_from_rfc3339("2026-07-03T00:00:00Z").unwrap().with_timezone(&Utc);
        let resets_at = now + Duration::hours(6 * 24);
        let base = (now - Duration::hours(6)).timestamp_millis();
        let samples = vec![
            sample(0, base, 40.0),
            sample(180, base, 55.0),
            sample(360, base, 70.0),
        ];
        let w = WindowState {
            kind: "weekly_all",
            scope_key: "all",
            scope_label: None,
            percent: 70.0,
            severity: None,
            resets_at: Some(resets_at),
        };
        let p = project(&w, &samples, now, &opts());
        assert!(p.alert_worthy, "70% this early is well beyond → alert: {}", p.summary);
    }

    #[test]
    fn mid_window_burst_alerts() {
        // Half-way through the window, burning fast → past the early phase, alert.
        let now = DateTime::parse_from_rfc3339("2026-07-03T00:00:00Z").unwrap().with_timezone(&Utc);
        let resets_at = now + Duration::hours(84); // 50% through 7d window
        let base = (now - Duration::hours(6)).timestamp_millis();
        let samples = vec![
            sample(0, base, 40.0),
            sample(180, base, 46.0),
            sample(360, base, 52.0),
        ];
        let w = WindowState {
            kind: "weekly_all",
            scope_key: "all",
            scope_label: None,
            percent: 52.0,
            severity: None,
            resets_at: Some(resets_at),
        };
        let p = project(&w, &samples, now, &opts());
        assert!(p.will_hit_wall, "should project a wall: {}", p.summary);
        assert!(p.alert_worthy, "mid-window burst should alert: {}", p.summary);
        assert!(p.cap_eta.is_some());
    }
}

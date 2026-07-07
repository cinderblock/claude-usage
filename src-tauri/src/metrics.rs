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
    /// Standard error of `rate_per_hour` from the same fit. None with fewer
    /// than 3 samples (no residual degrees of freedom).
    pub rate_stderr: Option<f64>,
    /// Hours until predicted to reach 100% (None if flat/declining or no rate).
    pub eta_to_100_hours: Option<f64>,
    /// When we predict hitting 100% (None if not projected to).
    pub cap_eta: Option<DateTime<Utc>>,
    /// Best estimate of utilization at reset time.
    pub projected_final_pct: f64,
    /// 10th–90th percentile band around `projected_final_pct`, propagating the
    /// velocity fit's uncertainty over the remaining time. None without a
    /// stderr (too few samples) or without a usable measured rate.
    pub projected_final_low_pct: Option<f64>,
    pub projected_final_high_pct: Option<f64>,
    /// Probability of hitting 100% at least `margin` before reset — the area
    /// under the rate distribution beyond the rate needed to cap that early,
    /// rather than a point comparison of the mean. None when no stderr.
    pub cap_probability: Option<f64>,
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

/// Least-squares fit of percent-vs-hours.
struct RateFit {
    /// Slope in %/hour.
    rate: f64,
    /// Standard error of the slope. None with fewer than 3 samples (no
    /// residual degrees of freedom to estimate noise from).
    stderr: Option<f64>,
}

/// Returns None if fewer than 2 distinct-time points or a degenerate time span.
fn fit_rate(samples: &[Sample]) -> Option<RateFit> {
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
    let rate = (n * sxy - sx * sy) / denom;
    let stderr = (samples.len() >= 3).then(|| {
        let intercept = (sy - rate * sx) / n;
        let sse: f64 = xs
            .iter()
            .zip(&ys)
            .map(|(x, y)| {
                let e = y - (intercept + rate * x);
                e * e
            })
            .sum();
        // Var(slope) = s² / Σ(x−x̄)², with Σ(x−x̄)² = denom/n.
        ((sse / (n - 2.0)) * n / denom).sqrt()
    });
    Some(RateFit { rate, stderr })
}

/// Standard normal CDF via the Abramowitz–Stegun erf approximation
/// (|error| < 1.5e-7 — far below what this signal needs).
fn normal_cdf(z: f64) -> f64 {
    let x = z / std::f64::consts::SQRT_2;
    let t = 1.0 / (1.0 + 0.3275911 * x.abs());
    let poly = t * (0.254829592
        + t * (-0.284496736 + t * (1.421413741 + t * (-1.453152027 + t * 1.061405429))));
    let erf = 1.0 - poly * (-x * x).exp();
    let erf = if x < 0.0 { -erf } else { erf };
    0.5 * (1.0 + erf)
}

/// Compute the projection for one window given its recent samples (already
/// filtered to the current window instance, oldest first) and config knobs.
pub struct ProjectOpts {
    pub margin_mins: i64,
    /// Below this fraction elapsed, projection warnings are suppressed as noise…
    pub min_elapsed_frac: f64,
    /// …unless current utilization is already at/above this.
    pub well_beyond_pct: f64,
    /// Only alert when `cap_probability` is at least this (0–1). Compares the
    /// area under the rate distribution, not just the mean, so a projection
    /// that barely crosses the wall on a noisy fit stays amber instead of red.
    pub cap_confidence: f64,
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

    let fit = fit_rate(samples);
    let rate_per_hour = fit.as_ref().map(|f| f.rate);
    let rate_stderr = fit.as_ref().and_then(|f| f.stderr);
    let margin_hours = opts.margin_mins as f64 / 60.0;

    // ETA + projected-final. Prefer measured velocity; fall back to even-pace
    // extrapolation from window start when we lack a usable rate.
    let mut eta_to_100_hours = None;
    let mut cap_eta = None;
    let mut projected_final_low_pct = None;
    let mut projected_final_high_pct = None;
    let mut cap_probability = None;
    let projected_final_pct;

    let usable_rate = rate_per_hour.filter(|r| *r > 0.01);
    if let Some(rate) = usable_rate {
        let eta = (100.0 - w.percent) / rate;
        if eta.is_finite() && eta >= 0.0 {
            eta_to_100_hours = Some(eta);
            cap_eta = Some(now + Duration::seconds((eta * 3600.0) as i64));
        }
        projected_final_pct = w.percent + rate * time_to_reset_hours;

        if let Some(se) = rate_stderr {
            // Slope uncertainty propagated over the remaining time. The current
            // percent is a measurement, not a fit, so it contributes no spread.
            let sd_final = se * time_to_reset_hours.max(0.0);
            const Z90: f64 = 1.2816; // 10th–90th percentile
            projected_final_low_pct = Some((projected_final_pct - Z90 * sd_final).max(w.percent));
            projected_final_high_pct = Some(projected_final_pct + Z90 * sd_final);

            // P(cap at least `margin` early) = P(true rate ≥ the rate that
            // reaches 100% by reset − margin).
            let lead = time_to_reset_hours - margin_hours;
            cap_probability = Some(if w.percent >= 100.0 {
                1.0
            } else if lead <= 0.0 {
                0.0
            } else {
                let needed = (100.0 - w.percent) / lead;
                if se < 1e-9 {
                    if rate >= needed { 1.0 } else { 0.0 }
                } else {
                    1.0 - normal_cdf((needed - rate) / se)
                }
            });
        }
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
    // This is the raw mean projection — computed and shown even early in a window.
    let will_hit_wall = match eta_to_100_hours {
        Some(eta) => eta < (time_to_reset_hours - margin_hours),
        None => false,
    };

    // Alert-worthiness gates the noise: early in a window the velocity fit is
    // unreliable (a small initial burst extrapolates to "will run out"), so we
    // don't actively warn unless we're past the early phase OR usage is already
    // well beyond where it should be. When the fit gives us an uncertainty,
    // also require the cap *probability* to clear the confidence bar — a mean
    // that barely crosses on a noisy fit isn't worth a red alert yet.
    let too_early = elapsed_frac < opts.min_elapsed_frac && w.percent < opts.well_beyond_pct;
    let confident = cap_probability.map_or(true, |p| p >= opts.cap_confidence);
    let alert_worthy = will_hit_wall && !too_early && confident;

    let summary = build_summary(
        w,
        will_hit_wall,
        cap_eta,
        time_to_reset_hours,
        projected_final_pct,
        rate_per_hour,
        cap_probability,
    );

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
        rate_stderr,
        eta_to_100_hours,
        cap_eta,
        projected_final_pct,
        projected_final_low_pct,
        projected_final_high_pct,
        cap_probability,
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
    cap_probability: Option<f64>,
) -> String {
    let name = w.scope_label.clone().unwrap_or_else(|| pretty_kind(w.kind));
    let odds = cap_probability
        .map(|p| format!(" — ~{:.0}% likely", p * 100.0))
        .unwrap_or_default();
    if will_hit_wall {
        if let (Some(cap), Some(_)) = (cap_eta, rate) {
            let to_cap = (cap - Utc::now()).num_seconds() as f64 / 3600.0;
            return format!(
                "{name}: at this pace you cap in {} (window resets in {}){odds}",
                fmt_dur(to_cap),
                fmt_dur(time_to_reset_hours)
            );
        }
        return format!("{name}: projected to hit the cap before reset{odds}");
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
            cap_confidence: 0.75,
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
        // Perfectly linear samples → ~zero stderr → certainty.
        assert!(p.cap_probability.unwrap() > 0.99);
        let (lo, hi) = (p.projected_final_low_pct.unwrap(), p.projected_final_high_pct.unwrap());
        assert!(hi - lo < 1.0, "clean fit should give a tight band: {lo}–{hi}");
    }

    /// Noisy samples around the same mean burn as `mid_window_burst_alerts`:
    /// the band widens and the cap probability drops below certainty.
    #[test]
    fn noisy_fit_widens_band_and_softens_probability() {
        let now = DateTime::parse_from_rfc3339("2026-07-03T00:00:00Z").unwrap().with_timezone(&Utc);
        let resets_at = now + Duration::hours(84);
        let base = (now - Duration::hours(6)).timestamp_millis();
        // Mean slope ~2%/hr but bursty: flat stretches and jumps.
        let samples = vec![
            sample(0, base, 40.0),
            sample(72, base, 40.5),
            sample(144, base, 47.0),
            sample(216, base, 47.5),
            sample(288, base, 48.0),
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
        assert!(p.rate_stderr.unwrap() > 0.0);
        let (lo, hi) = (p.projected_final_low_pct.unwrap(), p.projected_final_high_pct.unwrap());
        assert!(hi - lo > 10.0, "noisy fit should give a wide band: {lo}–{hi}");
        assert!(lo >= 52.0, "band low is clamped at current percent");
        let prob = p.cap_probability.unwrap();
        assert!(prob > 0.0 && prob < 1.0, "noisy fit → uncertain cap: {prob}");
    }

    /// The mean projection barely crosses the wall on a noisy fit: at the
    /// default confidence bar the alert is suppressed; at zero it fires.
    #[test]
    fn confidence_gates_alert_on_probability_not_mean() {
        let now = DateTime::parse_from_rfc3339("2026-07-03T00:00:00Z").unwrap().with_timezone(&Utc);
        // 26h left: needed rate ≈ 1.88%/hr vs fitted ≈ 1.98%/hr — the mean
        // crosses, but only just, and the fit noise makes it a coin-ish flip.
        let resets_at = now + Duration::hours(26);
        let base = (now - Duration::hours(6)).timestamp_millis();
        let samples = vec![
            sample(0, base, 40.0),
            sample(72, base, 40.5),
            sample(144, base, 47.0),
            sample(216, base, 47.5),
            sample(288, base, 48.0),
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
        assert!(p.will_hit_wall, "mean still projects a wall: {}", p.summary);
        let prob = p.cap_probability.unwrap();
        assert!(prob > 0.5 && prob < 0.75, "barely-crossing mean → middling odds: {prob}");
        assert!(!p.alert_worthy, "middling odds shouldn't clear the 0.75 confidence bar");

        let lax = ProjectOpts { cap_confidence: 0.0, ..opts() };
        let p = project(&w, &samples, now, &lax);
        assert!(p.alert_worthy, "zero confidence bar falls back to mean behavior");
    }
}

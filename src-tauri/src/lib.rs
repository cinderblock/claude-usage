pub mod alerts;
pub mod config;
pub mod credentials;
pub mod history;
pub mod metrics;
pub mod splat;
pub mod tray;
pub mod usage;

use anyhow::Result;
use chrono::{DateTime, Duration, Utc};
use config::Config;
use metrics::{Projection, WindowState};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{Emitter, Manager};
use tauri_plugin_notification::NotificationExt;

/// Floor on the poll cadence — the endpoint rate-limits chatty clients.
const MIN_POLL_SECS: u64 = 30;
/// Give up on the optional plan label after this many failed tries, so it
/// can't add a second request to every poll indefinitely.
const MAX_PLAN_ATTEMPTS: u8 = 5;

/// What the frontend + tray render from.
#[derive(Debug, Clone, Serialize)]
pub struct Snapshot {
    pub generated_at: DateTime<Utc>,
    pub plan: Option<String>,
    pub tray_percent: f64,
    pub tray_status: String,
    pub windows: Vec<Projection>,
    pub error: Option<String>,
}

pub struct AppState {
    client: reqwest::Client,
    history: history::History,
    config: Mutex<Config>,
    alerts: Mutex<alerts::AlertState>,
    latest: Mutex<Option<Snapshot>>,
    plan: Mutex<Option<String>>,
    /// Best-effort plan-label fetches so far; capped so a failing profile
    /// endpoint doesn't add a second request to every poll forever.
    plan_attempts: Mutex<u8>,
    /// Consecutive failed polls, for exponential backoff.
    consecutive_errors: Mutex<u32>,
    /// Seconds until the next scheduled poll — poll interval normally,
    /// a backoff delay after failures (respects 429 Retry-After).
    next_delay_secs: Mutex<u64>,
    config_dir: PathBuf,
}

/// Ensure a usable access token, refreshing + persisting if expired.
pub async fn ensure_token(client: &reqwest::Client, self_refresh: bool) -> Result<String> {
    let creds = credentials::load()?;
    if creds.is_expired(60) && self_refresh {
        let updated = credentials::refresh(client, &creds).await?;
        credentials::save(&updated)?;
        Ok(updated.access_token)
    } else {
        Ok(creds.access_token)
    }
}

fn status_for(p: &Projection, cfg: &Config) -> tray::Status {
    // Red follows the latched alert, not the raw flag — a noisy fit flapping
    // across the confidence bar shows amber until it sustains.
    if p.alert_engaged {
        tray::Status::Critical
    } else if p.alert_worthy
        || (p.percent >= cfg.near_cap_pct && p.rate_per_hour.map(|r| r > 0.01).unwrap_or(false))
        || matches!(p.severity.as_deref(), Some("warning") | Some("critical") | Some("exceeded"))
    {
        tray::Status::Warn
    } else {
        tray::Status::Ok
    }
}

fn status_label(s: tray::Status) -> &'static str {
    match s {
        tray::Status::Critical => "critical",
        tray::Status::Warn => "warn",
        tray::Status::Ok => "ok",
        tray::Status::Unknown => "unknown",
    }
}

fn worst(a: tray::Status, b: tray::Status) -> tray::Status {
    use tray::Status::*;
    let rank = |s: &tray::Status| match s {
        Critical => 3,
        Warn => 2,
        Ok => 1,
        Unknown => 0,
    };
    if rank(&a) >= rank(&b) {
        a
    } else {
        b
    }
}

/// First day of the next calendar month at 00:00:00 UTC. Used as the reset
/// anchor for the usage-billing pool, which the API reports without one.
fn next_month_start(now: DateTime<Utc>) -> DateTime<Utc> {
    use chrono::{Datelike, TimeZone};
    let (y, m) = (now.year(), now.month());
    let (ny, nm) = if m == 12 { (y + 1, 1) } else { (y, m + 1) };
    Utc.with_ymd_and_hms(ny, nm, 1, 0, 0, 0).single().unwrap_or(now)
}

/// Record one sample and project a single window from its recent history.
fn project_window(
    state: &AppState,
    now: DateTime<Utc>,
    cfg: &Config,
    kind: &str,
    scope_key: &str,
    scope_label: Option<String>,
    percent: f64,
    severity: Option<String>,
    resets_at: Option<DateTime<Utc>>,
) -> Projection {
    let window_len_h = metrics::window_len_hours(kind);
    let window_start_ms = resets_at
        .map(|r| (r - Duration::hours(window_len_h as i64)).timestamp_millis())
        .unwrap_or_else(|| (now - Duration::hours(window_len_h as i64)).timestamp_millis());

    // Record this sample, then read the current window's history for velocity.
    let _ = state.history.insert(
        now.timestamp_millis(),
        kind,
        scope_key,
        percent,
        resets_at.map(|r| r.timestamp_millis()),
    );

    // Only look back within the configured velocity window AND the current
    // window instance (avoid mixing a prior, since-reset series).
    let vel_start = (now - Duration::minutes((cfg.velocity_window_hours * 60.0) as i64)).timestamp_millis();
    let since = window_start_ms.max(vel_start);
    let samples = state.history.samples_since(kind, scope_key, since).unwrap_or_default();

    let w = WindowState {
        kind,
        scope_key,
        scope_label,
        percent,
        severity,
        resets_at,
    };
    let opts = metrics::ProjectOpts {
        margin_mins: cfg.projection_margin_mins,
        min_elapsed_frac: cfg.min_elapsed_frac,
        well_beyond_pct: cfg.well_beyond_pct,
        cap_confidence: cfg.cap_confidence,
    };
    metrics::project(&w, &samples, now, &opts)
}

/// Build the set of projections from a usage response + history.
fn build_projections(state: &AppState, usage: &usage::UsageResponse, now: DateTime<Utc>, cfg: &Config) -> Vec<Projection> {
    let mut projections = Vec::new();
    for l in &usage.limits {
        projections.push(project_window(
            state,
            now,
            cfg,
            &l.kind,
            &usage::Scope::key(&l.scope),
            usage::Scope::label(&l.scope),
            l.percent,
            l.severity.clone(),
            l.resets_at,
        ));
    }

    // Usage-based billing pool. Shown whenever it's enabled on the account
    // (extra_usage.is_enabled) — same as every other window, no separate
    // display toggle. Enabling/disabling the pool itself is done on claude.ai;
    // this app only ever reads its state. Modeled as a monthly window anchored
    // to the calendar-month boundary (the API gives no reset time).
    if let Some(eu) = usage.extra_usage.as_ref().filter(|e| e.is_enabled) {
        let resets_at = next_month_start(now);
        let mut p = project_window(
            state,
            now,
            cfg,
            "monthly_extra",
            "all",
            Some(metrics::pretty_kind("monthly_extra")),
            eu.utilization,
            None,
            Some(resets_at),
        );
        // Attach dollar figures for display (minor units → major).
        if let (Some(limit), Some(used)) = (eu.monthly_limit, eu.used_credits) {
            let decimals = eu.decimal_places.unwrap_or(2);
            let scale = 10f64.powi(decimals as i32);
            p.dollars = Some(metrics::Dollars {
                used: used / scale,
                limit: limit / scale,
                currency: eu.currency.clone().unwrap_or_else(|| "USD".into()),
                decimals,
            });
        }
        projections.push(p);
    }

    projections
}

fn build_menu(app: &tauri::AppHandle, snapshot: &Snapshot) -> tauri::Result<Menu<tauri::Wry>> {
    let mut info_items: Vec<MenuItem<tauri::Wry>> = Vec::new();
    if let Some(plan) = &snapshot.plan {
        info_items.push(MenuItem::with_id(app, "plan", format!("Claude — {plan}"), false, None::<&str>)?);
    }
    for p in &snapshot.windows {
        let name = p.scope_label.clone().unwrap_or_else(|| metrics::pretty_kind(&p.kind));
        let resets = fmt_hours(p.time_to_reset_hours);
        let flag = if p.alert_worthy { "  ⚠" } else { "" };
        let label = format!("{name}: {:.0}%  · resets {resets}{flag}", p.percent);
        info_items.push(MenuItem::with_id(app, format!("info_{}_{}", p.kind, p.scope_key), label, false, None::<&str>)?);
    }

    let sep1 = PredefinedMenuItem::separator(app)?;
    let refresh = MenuItem::with_id(app, "refresh", "Refresh now", true, None::<&str>)?;
    let open = MenuItem::with_id(app, "open", "Open window", true, None::<&str>)?;
    let settings = MenuItem::with_id(app, "settings", "Settings", true, None::<&str>)?;
    let sep2 = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;

    let mut refs: Vec<&dyn tauri::menu::IsMenuItem<tauri::Wry>> = Vec::new();
    for it in &info_items {
        refs.push(it);
    }
    refs.push(&sep1);
    refs.push(&refresh);
    refs.push(&open);
    refs.push(&settings);
    refs.push(&sep2);
    refs.push(&quit);

    Menu::with_items(app, &refs)
}

fn fmt_hours(hours: f64) -> String {
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

/// Run one poll cycle: fetch, project, alert, update tray + emit to UI.
pub async fn poll_once(app: tauri::AppHandle, state: Arc<AppState>) {
    let cfg = state.config.lock().unwrap().clone();
    let now = Utc::now();

    let mut snapshot = match run_fetch(&state, &cfg, now).await {
        Ok(s) => {
            *state.consecutive_errors.lock().unwrap() = 0;
            *state.next_delay_secs.lock().unwrap() = cfg.poll_interval_secs.max(MIN_POLL_SECS);
            s
        }
        Err(e) => {
            // Back off: double the interval per consecutive failure (capped at
            // 30 min); on 429 also respect Retry-After with a 5-minute floor.
            let errors = {
                let mut c = state.consecutive_errors.lock().unwrap();
                *c = c.saturating_add(1);
                *c
            };
            let base = cfg.poll_interval_secs.max(MIN_POLL_SECS);
            let mut delay = base.saturating_mul(1u64 << errors.min(6)).min(1800);
            let rate_limited = e.downcast_ref::<usage::RateLimited>().cloned();
            if let Some(ref rl) = rate_limited {
                delay = delay.max(rl.retry_after_secs.unwrap_or(0)).max(300);
            }
            *state.next_delay_secs.lock().unwrap() = delay;

            let error = if rate_limited.is_some() {
                format!(
                    "rate limited by the usage endpoint — waiting ~{} min before retrying",
                    delay.div_ceil(60)
                )
            } else {
                e.to_string()
            };

            // Keep showing the last good data (stale, with its original
            // timestamp so "updated X ago" reflects data age) under the banner.
            match state.latest.lock().unwrap().clone() {
                Some(prev) => Snapshot {
                    tray_status: "unknown".into(),
                    error: Some(error),
                    ..prev
                },
                None => Snapshot {
                    generated_at: now,
                    plan: state.plan.lock().unwrap().clone(),
                    tray_percent: 0.0,
                    tray_status: "unknown".into(),
                    windows: vec![],
                    error: Some(error),
                },
            }
        }
    };

    // Advance the alert latches and fire notifications (skip on error
    // snapshots). Runs even with notifications off so the latched red
    // state stays meaningful.
    if snapshot.error.is_none() {
        let alerts = {
            let mut st = state.alerts.lock().unwrap();
            let fired = st.evaluate(&snapshot.windows, &cfg, now.timestamp_millis());
            for p in &mut snapshot.windows {
                p.alert_engaged = st.proj_engaged(&p.kind, &p.scope_key);
            }
            fired
        };

        // Tray color is only known once engagement is set.
        let mut status = tray::Status::Ok;
        for p in &snapshot.windows {
            status = worst(status, status_for(p, &cfg));
        }
        snapshot.tray_status = status_label(status).into();

        if cfg.notifications_enabled {
            let icon = notification_icon(&app);
            for a in alerts {
                let mut b = app.notification().builder().title(a.title).body(a.body);
                if let Some(ref path) = icon {
                    b = b.icon(path.as_str());
                }
                let _ = b.show();
            }
        }
    }

    // Update tray icon, tooltip, menu.
    if let Some(tray) = app.tray_by_id("main") {
        let status = match snapshot.tray_status.as_str() {
            "critical" => tray::Status::Critical,
            "warn" => tray::Status::Warn,
            "ok" => tray::Status::Ok,
            _ => tray::Status::Unknown,
        };
        let _ = tray.set_icon(Some(tray::render_icon(status)));
        let _ = tray.set_tooltip(Some(tooltip(&snapshot)));
        if let Ok(menu) = build_menu(&app, &snapshot) {
            let _ = tray.set_menu(Some(menu));
        }
    }

    *state.latest.lock().unwrap() = Some(snapshot.clone());
    let _ = app.emit("usage-updated", &snapshot);

    // Keep history bounded (30 days).
    let _ = state
        .history
        .prune((now - Duration::days(30)).timestamp_millis());
}

async fn run_fetch(state: &AppState, cfg: &Config, now: DateTime<Utc>) -> Result<Snapshot> {
    let token = ensure_token(&state.client, cfg.self_refresh_tokens).await?;

    // Best-effort plan label (once; bounded retries).
    if state.plan.lock().unwrap().is_none() {
        let attempt = {
            let mut a = state.plan_attempts.lock().unwrap();
            *a = a.saturating_add(1);
            *a
        };
        if attempt <= MAX_PLAN_ATTEMPTS {
            if let Ok(Some(profile)) = usage::fetch_profile(&state.client, &token).await {
                let tier = profile
                    .organization
                    .get("rate_limit_tier")
                    .and_then(|v| v.as_str())
                    .map(prettify_tier);
                *state.plan.lock().unwrap() = tier;
            }
        }
    }

    let usage = usage::fetch_usage(&state.client, &token)
        .await?
        .ok_or_else(|| anyhow::anyhow!("token rejected (401) — run Claude Code to refresh"))?;

    // Limits parse leniently (malformed entries are dropped); if NONE survived,
    // the response was degraded — fail the poll so the stale-data path keeps
    // the last good snapshot instead of publishing an empty one (which would
    // also wipe the alert latches).
    if usage.limits.is_empty() {
        anyhow::bail!("usage response contained no parseable limit windows");
    }

    let projections = build_projections(state, &usage, now, cfg);

    // Tray percent = the session (5-hour) window; color = worst across all.
    let tray_percent = projections
        .iter()
        .find(|p| p.kind == "session")
        .map(|p| p.percent)
        .unwrap_or(usage.five_hour.utilization);
    Ok(Snapshot {
        generated_at: now,
        plan: state.plan.lock().unwrap().clone(),
        tray_percent,
        // Provisional: poll_once recomputes this once alert latches are set.
        tray_status: status_label(tray::Status::Ok).into(),
        windows: projections,
        error: None,
    })
}

fn prettify_tier(t: &str) -> String {
    // e.g. "default_claude_max_20x" -> "Max 20x"
    let t = t.replace("default_", "");
    if let Some(rest) = t.strip_prefix("claude_max_") {
        format!("Max {rest}")
    } else if t == "claude_max" {
        "Max".into()
    } else if t.contains("pro") {
        "Pro".into()
    } else {
        t.replace('_', " ")
    }
}

/// Absolute path to the bundled notification icon, if present. Windows toasts
/// otherwise fall back to a generic exe icon.
fn notification_icon(app: &tauri::AppHandle) -> Option<String> {
    let p = app.path().resource_dir().ok()?.join("icons/128x128.png");
    if p.exists() {
        Some(p.to_string_lossy().to_string())
    } else {
        None
    }
}

fn tooltip(s: &Snapshot) -> String {
    if let Some(e) = &s.error {
        return format!("Claude Usage — error: {e}");
    }
    let mut parts = Vec::new();
    for p in &s.windows {
        let name = p.scope_label.clone().unwrap_or_else(|| metrics::pretty_kind(&p.kind));
        parts.push(format!("{name} {:.0}%", p.percent));
    }
    format!("Claude Usage · {}", parts.join(" · "))
}

// ---- Tauri commands ----

#[tauri::command]
fn get_usage(state: tauri::State<'_, Arc<AppState>>) -> Option<Snapshot> {
    state.latest.lock().unwrap().clone()
}

#[tauri::command]
async fn refresh_now(app: tauri::AppHandle, state: tauri::State<'_, Arc<AppState>>) -> Result<(), String> {
    poll_once(app, state.inner().clone()).await;
    Ok(())
}

#[tauri::command]
fn get_config(state: tauri::State<'_, Arc<AppState>>) -> Config {
    state.config.lock().unwrap().clone()
}

#[tauri::command]
fn test_notification(app: tauri::AppHandle) -> Result<(), String> {
    let mut b = app
        .notification()
        .builder()
        .title("Claude Usage")
        .body("Test notification — this is how alerts will look.");
    if let Some(path) = notification_icon(&app) {
        b = b.icon(path);
    }
    b.show().map_err(|e| e.to_string())
}

#[tauri::command]
fn set_config(app: tauri::AppHandle, state: tauri::State<'_, Arc<AppState>>, config: Config) -> Result<(), String> {
    config.save(&state.config_dir).map_err(|e| e.to_string())?;
    *state.config.lock().unwrap() = config.clone();
    // Let any other open window (e.g. the main popup, if Settings is edited
    // live) pick up the change without waiting for its own next poll.
    let _ = app.emit("config-updated", &config);
    Ok(())
}

#[tauri::command]
fn open_settings_window(app: tauri::AppHandle) -> Result<(), String> {
    let win = app.get_webview_window("settings").ok_or("settings window missing")?;
    win.show().map_err(|e| e.to_string())?;
    win.set_focus().map_err(|e| e.to_string())?;
    Ok(())
}

fn toggle_window(app: &tauri::AppHandle) {
    if let Some(win) = app.get_webview_window("main") {
        if win.is_visible().unwrap_or(false) {
            let _ = win.hide();
        } else {
            use tauri_plugin_positioner::{Position, WindowExt};
            let _ = win.move_window(Position::TrayCenter);
            let _ = win.show();
            let _ = win.set_focus();
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // Must be the first plugin: a second launch focuses the running instance.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(win) = app.get_webview_window("main") {
                let _ = win.show();
                let _ = win.set_focus();
            }
        }))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_positioner::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .invoke_handler(tauri::generate_handler![
            get_usage,
            refresh_now,
            get_config,
            set_config,
            test_notification,
            open_settings_window
        ])
        .on_window_event(|window, event| match event {
            tauri::WindowEvent::CloseRequested { api, .. } => {
                // Hide, don't destroy: both windows reopen instantly next time
                // and the app keeps running via the tray.
                let _ = window.hide();
                api.prevent_close();
            }
            // Only the frameless popup hides on blur; Settings is a normal
            // window a user may want to leave open while referencing values
            // elsewhere.
            tauri::WindowEvent::Focused(false) if window.label() == "main" => {
                let _ = window.hide();
            }
            _ => {}
        })
        .setup(|app| {
            let handle = app.handle().clone();

            let config_dir = app.path().app_config_dir().unwrap_or_else(|_| PathBuf::from("."));
            let data_dir = app.path().app_data_dir().unwrap_or_else(|_| PathBuf::from("."));
            let cfg = Config::load(&config_dir);
            let history = history::History::open(&data_dir).expect("open history db");

            let initial_delay = cfg.poll_interval_secs.max(MIN_POLL_SECS);
            let state = Arc::new(AppState {
                client: usage::build_client().expect("http client"),
                history,
                config: Mutex::new(cfg),
                alerts: Mutex::new(alerts::AlertState::default()),
                latest: Mutex::new(None),
                plan: Mutex::new(None),
                plan_attempts: Mutex::new(0),
                consecutive_errors: Mutex::new(0),
                next_delay_secs: Mutex::new(initial_delay),
                config_dir,
            });
            app.manage(state.clone());

            // Tray with an initial placeholder icon + menu.
            let init_icon = tray::render_icon(tray::Status::Unknown);
            let init_menu = Menu::with_items(
                app,
                &[
                    &MenuItem::with_id(app, "refresh", "Refresh now", true, None::<&str>)?,
                    &MenuItem::with_id(app, "open", "Open window", true, None::<&str>)?,
                    &MenuItem::with_id(app, "settings", "Settings", true, None::<&str>)?,
                    &PredefinedMenuItem::separator(app)?,
                    &MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?,
                ],
            )?;

            TrayIconBuilder::with_id("main")
                .icon(init_icon)
                .tooltip("Claude Usage — starting…")
                .menu(&init_menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "refresh" => {
                        let app = app.clone();
                        if let Some(state) = app.try_state::<Arc<AppState>>() {
                            let state = state.inner().clone();
                            tauri::async_runtime::spawn(async move {
                                poll_once(app, state).await;
                            });
                        }
                    }
                    "open" => toggle_window(app),
                    "settings" => {
                        if let Some(win) = app.get_webview_window("settings") {
                            let _ = win.show();
                            let _ = win.set_focus();
                        }
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    let app = tray.app_handle();
                    tauri_plugin_positioner::on_tray_event(app, &event);
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        toggle_window(app);
                    }
                })
                .build(app)?;

            // Kick off the poll loop.
            let loop_state = state.clone();
            tauri::async_runtime::spawn(async move {
                loop {
                    poll_once(handle.clone(), loop_state.clone()).await;
                    // Set by poll_once: the poll interval normally, a backoff
                    // delay after failures.
                    let secs = (*loop_state.next_delay_secs.lock().unwrap()).max(MIN_POLL_SECS);
                    tokio::time::sleep(std::time::Duration::from_secs(secs)).await;
                }
            });

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

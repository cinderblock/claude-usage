# Claude Usage Watcher — Plan

Plan path: `~/.claude/plans/(scratch).md`
(On approval, copy to the project as `plans/claude-usage-watcher.md` per global CLAUDE.md, and keep it as the living plan.)

## Context

Opening the Claude webapp just to check subscription usage is heavy. We want a small
native tray app that continuously watches usage and only surfaces a custom UI/alerts when
something is worth acting on. The one hard unknown — how to read usage programmatically —
is **solved**: the same OAuth token Claude Code already stores locally can hit a clean JSON
endpoint. No webapp, no scraping, no browser automation.

## Data source (verified working)

- **Token**: `~/.claude/.credentials.json` → `claudeAiOauth.{accessToken, refreshToken, expiresAt, subscriptionType, rateLimitTier}`.
- **Usage endpoint**: `GET https://api.anthropic.com/api/oauth/usage`
  - Headers: `Authorization: Bearer <accessToken>`, `anthropic-beta: oauth-2025-04-20`, a `claude-cli`-style `User-Agent`.
  - Returns (verified live):
    - `five_hour`: `{ utilization, resets_at, ... }` — the 5-hour session window (was 82%).
    - `seven_day`: `{ utilization, resets_at, ... }` — the weekly window (was 13%).
    - `limits[]`: structured entries `{ kind, group, percent, severity, resets_at, scope, is_active }`.
      `severity` ∈ normal/warning/… ; `scope.model.display_name` gives per-model weekly (e.g. "Fable").
    - `extra_usage`, `spend` blocks (credits/overage) — often null, display if present.
- **Profile endpoint** (optional, for header/plan label): `GET https://api.anthropic.com/api/oauth/profile`
  → `account.has_claude_max`, `organization.rate_limit_tier` (e.g. `default_claude_max_20x`).

### Token freshness (design decision — safe default)
- **Default: read-only.** Re-read `.credentials.json` each poll. Claude Code refreshes the
  access token in normal use, so it's usually valid.
- If the access token is expired (`expiresAt` passed): attempt an OAuth refresh
  (`grant_type=refresh_token`, `client_id = 9d1c250a-e61b-44d9-88ed-5944d1962f5e` — Claude Code's
  public client id, confirmed as `application.uuid` in the profile response), then **atomically
  write the rotated tokens back** to `.credentials.json` so Claude Code stays in sync.
- **Gotcha (Things not to do):** refresh tokens may rotate. If we refresh and *don't* write back,
  Claude Code's stored refresh token goes stale and the user gets logged out. So: only refresh when
  actually expired, write back atomically (temp file + rename), and if a refresh fails, surface a
  "token stale — run Claude Code once" state rather than looping. Make self-refresh a setting
  (default on) so a cautious user can disable write-back entirely and just read the file.

## Decisions already made (don't re-ask)

- **Stack:** Tauri v2 (Rust backend + webview UI). Toolchain verified present: Rust 1.94,
  Node 24, WebView2 runtime, tauri-cli 2.10.
- **Surfaces (all of them):** tray icon with live %, native notifications, click-to-open popup
  detail window, and a right-click menu breakdown.
- **Alerts:** 5-hour crosses threshold; weekly high; use the API `severity` field; **and the headline
  feature — continuous weekly pace/burn-rate tracking** ("after 1 day, are we already well past 1/7
  of the way through?"), evaluated continuously, not daily.

## Feature: pace / burn-rate projection (the core signal)

**Alerting philosophy (user, explicit):** a raw threshold is *not* the signal. Being at 80%
when you're 90% through the window means you're *under* pace and will coast to the reset — no
warning. What matters is **velocity and whether that velocity hits a wall before the window
resets.** So projection is the primary alert; the raw % is informational only.

For any window with a known length (5-hour = 5h, seven_day = 7d; derive length from `kind`):
- `window_start = resets_at - window_len`; `elapsed_frac = clamp((now - window_start)/window_len, 0..1)`;
  `time_to_reset = resets_at - now`.
- **Recent velocity (primary, history-based):** from the stored time-series, fit `rate = d(util)/dt`
  over a recent trailing window (default last ~6h, but grow it if data is sparse; ignore the
  negative step at a reset). Then `eta_to_100 = (100 - util) / rate` when `rate > 0`.
  - **Alert = "will hit the wall before reset":** `rate > 0 && eta_to_100 < time_to_reset`.
    Severity scales with how early: `time_to_reset - eta_to_100` (bigger deficit = more urgent).
  - Report it as a human line: "weekly: at this pace you cap ~Thu 14:00, ~2 days before reset."
- **Even-pace fallback (no/scant history, e.g. first run):** `projected_final = util / elapsed_frac`;
  treat `projected_final > 100` the same way. Once enough history exists, prefer the measured rate —
  it catches a recent burst the since-start average would smear away.
- Apply to `seven_day` and each scoped weekly limit (per-model), and to `five_hour`.
- **Config:** `projection_margin` (how much lead time before predicted cap triggers the alert, e.g.
  warn once eta shows you capping ≥30 min early) rather than a raw-% threshold. Keep an optional
  hard "you are actually near the cap right now" nudge (e.g. util ≥ 95% *and* still burning) as a
  separate, lower-priority rule.

## Architecture

```
claude-usage/
  src-tauri/
    src/
      main.rs          # app bootstrap, tray, poll loop, Tauri commands/events
      credentials.rs   # read ~/.claude/.credentials.json; refresh + atomic write-back
      usage.rs         # HTTP client + serde models for /oauth/usage and /oauth/profile
      metrics.rs       # window_len, elapsed_frac, pace_ratio, projections, burn rate
      history.rs       # SQLite (rusqlite, bundled) append + range queries
      alerts.rs        # rule evaluation + de-dup/re-arm state machine
      tray.rs          # dynamic tray icon render + menu build
    Cargo.toml
    tauri.conf.json
  src/                 # Svelte + TS frontend (popup detail window)
    App.svelte, main.ts, lib/*  # bars, countdowns, pace, per-model scoped
  package.json, vite.config.ts, README.md, plans/
```

- **Poll loop** (Rust, tokio): every `poll_interval` (default 60s; manual "refresh now" command;
  optionally tighten to ~20s when any window is near a threshold). Each tick: ensure token →
  GET usage → parse → append to history → compute metrics → evaluate alerts → update tray icon +
  tooltip → emit `usage-updated` event to the frontend.
- **State**: latest snapshot + computed metrics + alert state held in a `tauri::State` (Mutex).
- **Commands** (frontend↔backend): `get_usage`, `refresh_now`, `get_history{range}`,
  `get_config`, `set_config`.
- **Events**: `usage-updated`, `alert-fired` pushed to the webview.

### Surfaces
- **Tray icon**: render the 5-hour % into a small RGBA image each poll (color-coded ring/badge:
  green→amber→red by severity/threshold) using `image` + `ab_glyph` with a bundled font. Tooltip =
  one-line summary (5h X% · 7d Y% · resets in …). Fallback if text-render is fiddly: pre-rendered
  icon buckets per 10% × severity color.
- **Right-click menu**: each limit `kind` with `percent` + reset countdown as text; "Refresh now",
  "Open", "Settings", "Quit".
- **Popup detail window**: frameless, always-on-top, small; toggled by left-clicking the tray,
  positioned near it. Shows every window (5-hour, 7-day, per-model scoped) with a progress bar,
  reset countdown, and the pace/projection line ("weekly projected 180% — cap ~Thu 14:00").
  Hidden (not quit) on blur/close.
- **Notifications**: `tauri-plugin-notification` toast on alert transitions.

### Config (`tauri-plugin-store` or a TOML in the app config dir)
`poll_interval` (60s), `projection_margin` (warn when projected to cap ≥ this lead time early,
default 30 min), `velocity_window` (trailing span for rate fit, default 6h), `near_cap_pct`
(secondary nudge, default 95), `use_api_severity` (default true), `self_refresh_tokens`
(default true), `launch_at_login`, notification toggles. Editable from the popup's Settings.

### Alert de-dup
Per rule, store last-fired level; fire only on transition into a worse state; re-arm when it
clears or when that window's `resets_at` advances (new window). Prevents per-poll spam.

## Build milestones

1. **Scaffold**: `npm create tauri-app` (Svelte+TS) into the empty dir; `git init`; confirm
   `cargo tauri dev` opens a blank window. Add plugins: notification, store, autostart,
   positioner, single-instance.
2. **Backend core**: `credentials.rs` (read + models) → `usage.rs` (client + models) →
   print a parsed snapshot to the log. This is the riskiest integration; validate first.
3. **History + metrics**: SQLite schema `(ts, kind, scope, percent, resets_at)`; `metrics.rs`
   pace/projection; unit tests with synthetic series.
4. **Tray**: icon render + tooltip + menu; wire the poll loop and manual refresh.
5. **Popup UI**: Svelte window rendering the snapshot + metrics via command/event; countdowns;
   settings panel.
6. **Alerts**: rule engine + de-dup + notifications; map API `severity` and pace rules to toasts.
7. **Token refresh + write-back**: implement expired-token path with atomic write; guard behind
   `self_refresh_tokens`.
8. **Polish**: autostart, single-instance, icon states, README, package with `cargo tauri build`.

## Verification

- **Endpoint smoke test** (already passing): a tiny script reads the token and prints the parsed
  `five_hour`/`seven_day`/`limits` — reuse as a Rust integration test / `--once` CLI mode.
- **Live run**: `cargo tauri dev` → tray shows the real current 5-hour % matching what the Claude
  webapp shows; hover tooltip and menu match; click opens the popup with correct bars/countdowns.
- **Pace logic**: unit tests over synthetic history (even pace → ratio ≈ 1; front-loaded burst →
  ratio > 1 and `projected_final > 100` with a sane ETA).
- **Alerts**: temporarily lower thresholds so a toast fires on the current real numbers; confirm it
  fires once (not every poll) and re-arms after reset.
- **Token refresh**: simulate expiry (set `expiresAt` in the past in a *copy*) and confirm refresh +
  atomic write-back produce a valid token without corrupting the file.

## Findings / gotchas

- **Debug binary + devUrl:** running `cargo run` (debug) makes the webview load `devUrl`
  (`localhost:1420`); with no Vite server that's `ERR_CONNECTION_REFUSED`. Use `npm run tauri dev`
  for dev, or a real `tauri build` (embeds `frontendDist`). Not a code bug.
- **Two binaries → ambiguous `cargo run`:** the `smoke` bin made `tauri dev` fail with "could not
  determine which binary to run". Fixed with `default-run = "claude-usage"` in `[package]`.
- **Alerting refinement (user):** separate the *projection* from the *alert*. `will_hit_wall` is the
  raw projection — always computed and shown (early-window shows a muted "on pace to cap early —
  monitoring"). `alert_worthy` gates notifications + red tray: suppressed while `elapsed_frac <
  min_elapsed_frac` (default 0.15) UNLESS `percent >= well_beyond_pct` (default 60). This stops
  noisy "will run out" alarms from a small burst at the very start of a window.
- **Projection uncertainty (user, 2026-07-07):** don't judge "too much too fast" on the mean alone.
  The least-squares fit now also returns the slope's standard error; from it we derive a 10–90%
  band on `projected_final_pct` and a `cap_probability` (normal CDF of the rate needed to cap
  ≥ margin early). `alert_worthy` additionally requires `cap_probability >= cap_confidence`
  (config, default 0.75); with < 3 samples there's no stderr and gating falls back to the mean.
  Caveat: the linear+iid-noise model understates real burstiness, but SSE over a staircase-y
  series does capture much of it — "simple confidence", by request.
- **Bars show overshoot (user, 2026-07-07):** popup bars span 0–150% with the 100% cap tick at the
  2/3 mark; a translucent "ghost" fill runs current → projected, the confidence band overlays it,
  and the region past the tick is red-tinted. Values are clamped at 150% for display.
- **Alert debounce + latch (user, 2026-07-07):** the old fire-once-until-clear de-dup re-armed on
  every single clear poll, so noisy projections hovering at the threshold toasted constantly. Each
  rule in `alerts.rs` is now a latch: condition must hold continuously `alert_sustain_mins`
  (default 10) to engage/notify, must be clear that long to release, and within one window
  instance re-fires only on escalation (proj: cap-probability +0.15; near: +5%; sev: rank +1).
  Window reset re-arms. `Projection.alert_engaged` carries the latched state to the tray + UI so
  red is stable too (`alert_worthy` alone now renders amber). Latches advance every successful
  poll even with notifications disabled.
- **429 backoff (user, 2026-07-08):** the usage endpoint rate-limited us ("rate_limit_error").
  Causes: 60s polling with NO backoff on errors (kept hammering while limited), plus the plan-label
  profile fetch retrying on every poll until it succeeded (2 req/poll when degraded). Fixes:
  typed `usage::RateLimited` error carrying Retry-After; exponential backoff on consecutive
  failures (base×2^k, cap 30 min; 429 floor 5 min, respects Retry-After); poll default 120s with
  a 30s floor (`MIN_POLL_SECS`); plan lookup capped at 5 attempts (`MAX_PLAN_ATTEMPTS`); error
  snapshots retain the last good windows + generated_at so the UI shows stale bars under the
  banner instead of going blank; the "run Claude Code" hint only shows for token errors.
  Note: the loop reads `next_delay_secs` (set by poll_once) instead of the config interval.

## Progress log

- [x] M1 Scaffold — Tauri v2 + SvelteKit-TS in `the project directory`; plugins added
  (notification, positioner, autostart). Window is hidden/frameless/always-on-top popup.
- [x] M2 Backend core — `credentials.rs` + `usage.rs`; **validated live** via `cargo run --bin smoke`
  (parsed session/weekly/weekly-scoped correctly).
- [x] M3 History + metrics — SQLite history; `metrics.rs` projection engine; **2 unit tests pass**,
  incl. the "80% but 90% through → no warning" case.
- [x] M4 Tray — color-coded badge (renders live %, loads a Windows font, ring fallback) + dynamic menu.
- [x] M5 Popup UI — `+page.svelte`: bars, live countdowns, projection lines, settings panel. Frontend builds.
- [x] M6 Alerts — velocity-based rule engine with de-dup/re-arm; wired to OS notifications.
- [x] M7 Token refresh + atomic write-back — implemented behind `self_refresh_tokens`.
- [x] Full app compiles and **launches without panic** (`claude-usage.exe` running; tray live).
- [ ] M8 Polish remaining: autostart **UI toggle** + auto-enable, single-instance guard, packaged
  installer (`npm run tauri build`), app icon set. Verify popup visually + notification fires
  (lower a threshold to force one).
- [ ] Copy this plan into project `plans/claude-usage-watcher.md` and (if desired) `git init` + commit.

## Things not to do

- Don't scrape the webapp or drive a browser — the JSON endpoint is authoritative.
- Don't refresh tokens without writing the rotated pair back atomically (would log Claude Code out).
- Don't print/log the access or refresh tokens.
- Don't poll aggressively (respect the endpoint; 60s baseline is plenty).
- Don't `git init`-and-commit or touch anything outside this project dir without asking.

## Open questions (non-blocking; sensible defaults chosen)

1. Default thresholds — 5h 80%, weekly pace factor 1.4. Fine to tune later in Settings.
2. Poll interval — 60s default. OK?
3. Icon style — number-in-badge vs colored ring. Will prototype the number badge; easy to swap.

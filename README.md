# Claude Usage

A small native (Tauri) tray app that watches your Claude subscription usage and
surfaces a custom UI only when it matters — no need to open the webapp just to
check where you stand.

## What it does

- Lives in the system tray with a color-coded badge showing your live **5‑hour**
  utilization.
- Left‑click the tray → a compact popup with every limit window (5‑hour, weekly,
  and per‑model weekly), each with a bar, a live reset countdown, and a
  projection line. Bars span 0–150% with the cap marked at the 2/3 point, so an
  overshooting projection (ghost fill + marker, with a likely‑range band) stays
  visible past 100%.
- Right‑click → a menu breakdown of every window + Refresh / Open / Quit.
- Native notifications when a window is **projected to run out before it resets**.

### Alerting is velocity-based, not threshold-based

A raw percentage is deliberately *not* the alarm. Being at 80% when you're 90%
through a window means you're under pace and will coast to the reset — no warning.
The app estimates your recent **burn velocity** from a local history of samples and
warns only when that velocity is projected to hit the cap *before* the window
resets (with a configurable lead time). The API's own `severity` and a
near‑cap nudge are secondary signals.

## How it reads usage

It reuses the OAuth token Claude Code already stores at
`~/.claude/.credentials.json` and calls `GET https://api.anthropic.com/api/oauth/usage`
— the same data the webapp shows. No scraping, no separate login.

If the access token is expired it can refresh it (using Claude Code's public
OAuth client id) and write the rotated token back atomically so Claude Code stays
in sync. This is on by default and can be disabled in Settings
(`self_refresh_tokens`).

## Architecture

- `src-tauri/src/credentials.rs` — read/refresh/persist the local OAuth token.
- `src-tauri/src/usage.rs` — HTTP client + models for the usage/profile endpoints.
- `src-tauri/src/history.rs` — SQLite time-series of samples (for velocity).
- `src-tauri/src/metrics.rs` — pace/projection engine (the core signal).
- `src-tauri/src/alerts.rs` — de-duplicated alert rules (re-arm on window reset).
- `src-tauri/src/tray.rs` — renders the color-coded tray badge.
- `src-tauri/src/lib.rs` — poll loop, tray/menu, Tauri commands + events.
- `src/routes/+page.svelte` — the popup UI.

## Develop

```sh
npm install
npm run tauri dev        # live-reload dev
cargo run --example smoke    # (in src-tauri/) print a parsed usage snapshot, no UI
cargo test --lib         # (in src-tauri/) projection unit tests
```

## Build

```sh
npm run tauri build
```

## Config

Stored at the app config dir (`config.json`). Editable from the popup's Settings:
`poll_interval_secs` (60), `projection_margin_mins` (30), `velocity_window_hours`
(6), `near_cap_pct` (95), `cap_confidence` (0.75), `use_api_severity`,
`self_refresh_tokens`, `notifications_enabled`.

### Projection uncertainty

The burn-velocity fit also yields a standard error, which propagates into a
10th–90th percentile band around the projected final % (shown on the bar) and a
**cap probability** — the area under the rate distribution beyond the rate that
would hit the cap early. Alerts require that probability to reach
`cap_confidence`, so a mean projection that barely crosses the wall on a noisy
fit shows amber ("on pace… ~60% odds") instead of going straight to red.

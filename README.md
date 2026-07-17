# Claude Usage

A small native (Tauri) tray app that watches your Claude subscription usage and
surfaces a custom UI only when it matters — no need to open the webapp just to
check where you stand.

## What it does

- Lives in the system tray with a color-coded badge showing your live **5‑hour**
  utilization.
- Left‑click the tray → a compact popup with every limit window (5‑hour, weekly,
  per‑model weekly, and the billing pool), each drawn as a usage-over-time
  chart: the actual usage line, a dashed even‑pace diagonal (above it = burning
  too fast), the projected path to the reset with its likely‑range band, and a
  live reset countdown. Windows on the same timescale share one chart.
- Right‑click → a menu breakdown of every window + Refresh / Open / History /
  Settings / Check for updates / Quit.
- A gear icon (or the tray's **Settings** menu item) opens Settings in its own
  resizable window, separate from the popup.
- A clock icon (or the tray's **History** menu item) opens the **History**
  window: pick any usage window (5‑hour, weekly, per‑model, billing) and see
  one bar per completed period — the peak % it reached — over 30 days / 90
  days / 1 year / all time. Click a bar to drill into that period's raw
  usage curve. Samples are kept forever by default (see
  [History retention](#history-retention)).
- Native notifications when a window is **projected to run out before it resets**.

### Alerting is velocity-based, not threshold-based

A raw percentage is deliberately *not* the alarm. Being at 80% when you're 90%
through a window means you're under pace and will coast to the reset — no warning.
The app estimates your recent **burn velocity** from a local history of samples and
warns only when that velocity is projected to hit the cap *before* the window
resets (with a configurable lead time). The API's own `severity` and a
near‑cap nudge are secondary signals.

### Usage-based billing

If you have usage-based ("extra") billing enabled on your account, its monthly
credit pool shows up automatically as its own window — spent vs. limit in
dollars, percent used, and the same projection treatment, except its chart caps
at 100% (a real dollar cap, not a rolling window that can run over). There
is no in-app control for enabling/disabling the pool or changing its dollar
limit — both are account-level billing decisions made on claude.ai. A
**Change limit ↗** link opens `claude.ai/new#settings/usage` for that. The pool
is anchored to the calendar-month boundary since the endpoint reports no reset
time for it.

## Scheduled messages & 5‑hour window priming

Beyond watching usage, the app can *send* to Claude on a schedule — via the local
Claude Code CLI (`claude -p`), not a raw API call, so it reuses Claude Code's own
auth and the send counts toward your usage exactly like normal CLI use.

- **Scheduled messages** — a list of prompts, each with a time, weekday set,
  model, and an optional "skip if a 5‑hour window is already active" gate. A
  **Send now** button fires any row immediately for testing.
- **Window priming** — a dedicated mode that sends a tiny **Haiku** message at
  chosen anchor times so a fresh **5‑hour session window starts early**. Because
  the 5‑hour window is anchored to your *first* message, priming lets you line
  the windows up with your day and fit **3 windows in a day instead of 2**. A
  draggable 24‑hour timeline in Settings shows the primed blocks (blue), your
  current live window (green), and now (dashed) — drag the anchor to place them.
  Slots are spaced 5h **plus a few seconds of slack** so a prime always lands
  just *after* the previous window resets, never on the boundary. A slot is
  skipped when a window is already running (nothing to start), and every prime is
  **verified** afterward by re‑polling usage to confirm the window actually went
  active — unverified sends are flagged in the Settings "Recent sends" log.

Sends go through whatever `claude` binary is on your `PATH` (override the path in
Settings if needed). Priming deliberately shapes when your rate‑limit windows
start; it's ordinary personal automation of something you could type by hand, but
whether to do it is your call under Anthropic's usage policy.

## Install

**Windows** — grab the latest installer (`.exe` NSIS or `.msi`) from
[Releases](https://github.com/cinderblock/claude-usage/releases).

**macOS** — grab the universal `.dmg` (Apple Silicon + Intel) from the same
page and drag the app to Applications. The build is only ad-hoc signed (no
Apple Developer certificate), so Gatekeeper refuses the first launch; clear
the quarantine flag once:

```sh
xattr -cr "/Applications/Claude Usage.app"
```

After that the app keeps itself current on both platforms: it checks for a new
signed release shortly after launch and then daily, installs it, and restarts —
or on demand via the tray's **Check for updates**. (The updater's own downloads
aren't quarantined, so the `xattr` dance is first-install only.) Windows is
well tested; macOS support is new. Linux is untested but likely just a matter
of adding a CI target.

## How it reads usage

It reuses the OAuth token Claude Code already stores locally — at
`~/.claude/.credentials.json` on Windows/Linux, in the login Keychain (service
`Claude Code-credentials`) on macOS — and calls
`GET https://api.anthropic.com/api/oauth/usage` — the same data the webapp
shows. No scraping, no separate login.

On macOS the first read pops a Keychain consent dialog asking to allow access
to Claude Code's item; choose **Always Allow** so the every-2-minutes poll
doesn't re-prompt.

If the access token is expired it can refresh it (using Claude Code's public
OAuth client id) and write the rotated token back — atomically for the file,
in place for the Keychain item — so Claude Code stays in sync. This is on by
default and can be disabled in Settings (`self_refresh_tokens`).

## Architecture

- `src-tauri/src/credentials.rs` — read/refresh/persist the local OAuth token
  (`.credentials.json` file on Windows/Linux, login Keychain on macOS).
- `src-tauri/src/usage.rs` — HTTP client + models for the usage/profile endpoints.
- `src-tauri/src/history.rs` — SQLite time-series of samples (velocity fits,
  long-term history, retention/downsampling).
- `src-tauri/src/metrics.rs` — pace/projection engine (the core signal).
- `src-tauri/src/alerts.rs` — de-duplicated alert rules (re-arm on window reset).
- `src-tauri/src/sender.rs` — sends a message by shelling out to `claude -p`
  (locates the binary, runs it hermetically, summarizes the JSON result).
- `src-tauri/src/schedule.rs` — pure `due()` evaluator + persisted fire-state
  (`schedule_state.json`) for scheduled messages and window priming.
- `src-tauri/src/tray.rs` — renders the color-coded tray badge.
- `src-tauri/src/lib.rs` — poll loop, tray/menu, windows, Tauri commands + events.
- `src/routes/+page.svelte` — root page for BOTH windows (see below); renders
  the popup or delegates to `src/lib/SettingsPanel.svelte` based on which
  window it's running in.

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

## CI & releases

- `ci.yml` — every push/PR: `svelte-check` + frontend build (ubuntu), `cargo
  test` (windows + macos).
- `release.yml` — push a `v*` tag matching the version in `tauri.conf.json` /
  `Cargo.toml` and it builds the Windows installers and the universal macOS
  `.dmg`, signs the updater artifacts (repo secrets `TAURI_SIGNING_PRIVATE_KEY`
  + `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`), and publishes a GitHub Release
  including the `latest.json` manifest the in-app updater polls.

## Windows

Three windows, all defined in `tauri.conf.json` and hidden at startup:
- **`main`** — the frameless, always-on-top popup toggled by clicking the tray
  icon; hides on blur.
- **`settings`** — a normal, resizable, native-chrome window opened via the
  popup's gear icon or the tray menu's "Settings" entry; stays open on blur
  since you may want to leave it up while checking values elsewhere.
- **`history`** — a larger resizable window (popup clock icon or the tray's
  "History") graphing long-term usage: peak-per-period bars with click-to-drill
  raw curves. All hide (rather than close) so reopening is instant and the app
  keeps running via the tray.

Since the frontend is SPA-only (`ssr = false` + a static fallback shell —
see [SvelteKit docs](https://svelte.dev/docs/kit/single-page-apps)), both
windows load the same root page; `src/routes/+page.svelte` checks
`getCurrentWindow().label` to decide whether to render the popup or delegate
to `SettingsPanel.svelte`.

## Config

Stored at the app config dir (`config.json`). Editable from the Settings window:
`poll_interval_secs` (120), `projection_margin_mins` (30), `velocity_window_hours`
(6), `near_cap_pct` (95), `cap_confidence` (0.75), `alert_sustain_mins` (10),
`use_api_severity`, `self_refresh_tokens`, `notifications_enabled`, plus the
history retention settings below. Saving emits a `config-updated` event so the
popup (if also open) picks up the change immediately instead of waiting for its
next poll.

Sending adds `claude_binary_path` (blank = autodetect `claude`),
`scheduled_messages` (a list of `{id, enabled, time_of_day, days, message,
model, only_if_session_inactive}`), and `priming` (`{enabled, anchor_time,
windows_per_day, slot_slack_secs, model, end_of_day, prime_prompt}`). A separate
`schedule_state.json` tracks what has already fired so restarts / wake‑from‑sleep
don't double‑fire or replay a stale slot (a due slot only fires within a 30‑min
grace window).

### History retention

Every poll's samples (all windows) are kept in `history.db` **forever by
default** — they power both the live velocity fits and the History window.
Two optional bounds, set in Settings:

- `history_retention_mode` (`unlimited` | `time` | `size`) with
  `history_retention_days` / `history_retention_mb` — cap the store by age or
  by on-disk size. Settings shows the live store size and, from the observed
  growth rate, estimates the counterpart (a day cap → expected MB; an MB cap →
  expected days of history).
- `history_downsample` (off by default) + `history_downsample_after_days` (60)
  — thin samples older than the cutoff to one peak-preserving point per hour
  per window instance (~30× smaller for old 5‑hour data) while recent data
  keeps full poll fidelity. Peaks survive, so the History window's
  per-period bars are unaffected.

At ~2‑minute polls the raw store grows roughly 50 MB/year, so "keep
everything" is a fine default.

### Projection uncertainty

The burn-velocity fit also yields a standard error, which propagates into a
10th–90th percentile band around the projected final % (shown on the chart) and a
**cap probability** — the area under the rate distribution beyond the rate that
would hit the cap early. Alerts require that probability to reach
`cap_confidence`, so a mean projection that barely crosses the wall on a noisy
fit shows amber ("on pace… ~60% odds") instead of going straight to red.

### Alerts are debounced and latched

Noisy fits flap across thresholds poll-to-poll, so a raw edge trigger would
toast constantly. Each alert rule is a latch instead: the condition must hold
continuously for `alert_sustain_mins` (10) before it engages and notifies, and
it must stay clear that long before it releases. Once fired for a window
instance it re-fires only on a real escalation (cap odds +0.15, usage +5%, or
an API severity step up) or after the window resets. The red tray/UI state
follows the latch too, so it doesn't flicker; amber shows while a projection is
still being confirmed.

### Logs

Written to the app log dir — on Windows
`%LOCALAPPDATA%\com.cinderblock.claude-usage\logs\claude-usage.log`, on macOS
`~/Library/Logs/com.cinderblock.claude-usage/claude-usage.log` — and
rotated at ~2 MB, keeping the newest 4 files (older ones are pruned at
startup). Poll results are logged at debug, poll failures (with backoff and,
for parse errors, the start of the offending body) at warn, alert latch
engage/release and fired notifications at info.

### Polite polling

Polls every `poll_interval_secs` (120, floor 30). On any fetch failure the
loop backs off exponentially (up to 30 min); a 429 also respects `Retry-After`
with a 5-minute floor. While errored, the popup keeps showing the last good
data (with its original "updated X ago" age) under the error banner, and the
optional plan-label lookup gives up after a few failures instead of adding a
second request to every poll.

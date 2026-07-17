# Usage History Window

## Goal

Stop throwing away usage datapoints after 30 days. Keep the full history of
every window (5-hour, weekly overall/per-model, and monthly billing) and add a
separate, openable **History** window that graphs old periods:

- **Summary view** (default): one point per completed window instance — the
  *peak %* it reached — so you see the trend of how hard each 5h block / week /
  month ran.
- **Drill-in**: click a period to see its raw sample sawtooth (the existing
  ramp-then-reset line) for just that instance.

Retention is user-controlled: keep everything by default, with optional
downsampling (default OFF) and an optional retention cap by **time or size**,
each showing an estimate of the other.

## Environment / context

- Tauri app. Frontend SvelteKit (SPA, `ssr=false`), Rust backend.
- History store: SQLite `history.db`, table `samples(ts, kind, scope, percent, resets_at)`.
  - `ts` epoch ms, `percent` is relative to the *current rolling window* (sawtooths to 0 each reset).
  - `resets_at` epoch ms — the natural **instance key**: all samples of one window instance share it.
    - `monthly_extra` uses `next_month_start(now)` as `resets_at` (non-null). Session/weekly come from the API.
- Key files:
  - `src-tauri/src/history.rs` — SQLite queries. **Add** `window_summaries`, `stats`; **change** prune.
  - `src-tauri/src/config.rs` — Config struct + defaults. **Add** retention/downsample fields.
  - `src-tauri/src/lib.rs` — poll loop (`poll_once`), prune call at ~L439, Tauri commands, tray menu, window open. **Add** history commands + `open_history_window` + menu item + downsample/retention prune.
  - `src-tauri/tauri.conf.json` — window defs. **Add** `history` window (hidden, resizable, decorated — like `settings`).
  - `src/lib/usage.ts` — TS types + `invoke` wrappers. **Add** summary/stats types + commands + Config fields.
  - `src/routes/+page.svelte` — branches on window label. **Add** `history` branch.
  - `src/lib/HistoryPanel.svelte` — NEW. The history UI (summary chart + drill-in).
  - `src/lib/SettingsPanel.svelte` — **Add** retention/downsample controls with the size/time estimate.
  - `src/lib/UsageChart.svelte` — reuse for drill-in; may add a summary/bar chart (new small component).

## Decisions already made (don't re-ask)

1. **History view = Both**: peak-per-window summary is the default; drill into any period's raw sawtooth.
2. **Retention = keep everything by default.** Provide user options:
   - Downsampling toggle, **default OFF**.
   - Retention cap selectable by **time (days)** OR **size (MB)**, showing an estimate of the other from the observed growth rate. "Unlimited" is the default.
3. **All windows** get long-term retention (5h, weekly overall + per-model, monthly billing).
4. New **dedicated window** (mirrors the Settings window pattern), opened from tray menu + a popup header button.

## Plan / steps

1. [x] **Backend retention/config**: Config fields (`history_retention_mode`,
   `history_retention_days`, `history_retention_mb`, `history_downsample`,
   `history_downsample_after_days`) + `RetentionMode` enum. Defaults = unlimited / off.
2. [x] **history.rs**: `window_summaries` (GROUP BY resets_at → peak/first/last/count),
   `stats()` (rows, span, page_count×page_size bytes), `downsample_before`
   (hourly peak-preserving thin via ROW_NUMBER window fn), `prune_to_size`
   (estimate rows to shed + VACUUM). Unit tests added (4, all passing).
3. [x] **lib.rs**: `apply_retention` replaces the fixed 30-day prune; commands
   `get_window_summaries`, `get_history_stats`, `open_history_window` registered;
   tray "History" item in both menu builders + handler.
4. [x] **tauri.conf.json**: `history` window (720×560, resizable, hidden).
5. [x] **capabilities/default.json**: added `history` to `windows` — without this
   every `invoke()` from the new window is silently permission-denied.
6. [x] **usage.ts**: `WindowSummary`, `HistoryStats`, `RetentionMode` types,
   command wrappers, Config fields.
7. [x] **HistoryPanel.svelte**: series picker, range picker (30d/90d/1y/all),
   peak-per-instance bar chart (month gridlines, ≥100% bars red), hover caption,
   click-to-drill raw sawtooth via existing UsageChart. `+page.svelte` branches
   on window label (now generic `windowLabel`, popup header gained 🕑 button).
8. [x] **SettingsPanel.svelte**: History section — store size/rows/since stat,
   retention mode select, days/MB inputs with cross-estimates from observed
   bytes/day, downsample toggle + cutoff. Also converted all `title=` tooltips
   to inline hint text (global UI rule).
9. [x] **README**: History window + retention section, window list updated.
10. [x] **Verify**: cargo test ✅ (20), svelte-check ✅, vite build ✅, and a live
    dev run against the real DB (prod app stopped, history window temporarily
    `visible:true`, screenshots): window renders, series dropdown lists all
    windows, summary shows 23 periods (matches the 22 real transitions in the
    data), drill-in click renders the raw curve ("Jul 15, 12 AM–5 AM · peak
    57% · 144 samples"). Prod app relaunched after.

## Findings / gotchas

- **`resets_at` jitters ±1–2 min between polls of the SAME window** (e.g.
  19:59 ↔ 20:00), so `GROUP BY resets_at` exploded 22 real 5h sessions into
  2603 "instances" on real data. Real transitions jump by hours. Fix: fold
  samples in ts order in Rust; new instance only when resets_at moves > 10 min.
  Same fix applied to downsampling's partition (hour-bucketed resets_at).
- **`resets_at IS NULL` ⇔ `percent == 0` ⇔ no active window** (3620 of 6382
  session samples). These are idle gaps, not instances — skipped during
  grouping, and they close any open instance.
- **Tauri capabilities are per-window-label**: `capabilities/default.json` listed
  only `["main", "settings"]`. A new window compiles + opens fine but every
  `invoke()` from it fails at runtime until its label is added. Caught pre-commit.
- Repo is NOT rustfmt-formatted (cargo fmt --check diffs in untouched files) —
  don't run `cargo fmt`, match surrounding style by hand.
- Production app auto-runs from `C:\Users\camer\AppData\Local\Claude Usage\claude-usage.exe`;
  it must be stopped before `npm run tauri dev` (same single-instance identifier,
  shared config.json + history.db) and relaunched after.

- `percent` is per-rolling-window, so a continuous plot is a sawtooth. The
  **peak per instance** (max percent grouped by `resets_at`) is the clean summary.
- `resets_at` is stored and non-null for all current window kinds → reliable
  instance key. (Guard for null just in case: fall back to bucketing by ts.)
- Size-based prune: SQLite file size only shrinks after VACUUM. Approach: estimate
  bytes/row from file size ÷ row count, delete oldest beyond the target row count,
  VACUUM occasionally (cost — do sparingly, e.g. only when far over).
- Volume: ~5 windows × ~720 samples/day ≈ 3.6k rows/day ≈ ~1.3M/yr, ~50 MB/yr raw.
  Fine for SQLite; downsampling (hourly) cuts old 5h data ~30×.

## Things not to do

- Don't prune in a way that deletes the *current* window's in-progress samples
  (retention/downsample must only touch data older than the active instance).
- Don't break the existing per-window popup charts (they call `get_history`).
- Don't add `title=` tooltips anywhere (global rule).

## Progress log

- [x] Explored codebase, confirmed data model + prune point, gathered decisions.
- [x] Implemented backend (config, history queries, retention, commands, tray).
- [x] Implemented frontend (HistoryPanel, Settings retention UI, popup button).
- [x] Fixed jitter-grouping bug found during live verification.
- [x] Verified end-to-end against real DB; prod app restored.
- [x] Committed.

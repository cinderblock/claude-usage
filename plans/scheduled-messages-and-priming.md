# Scheduled messages + 5-hour-window priming

Living plan. Approved 2026-07-17. Mirrors
`~/.claude/plans/sleepy-beaming-cosmos.md` with the user's refinements folded in.

## Goal

Let `claude-usage` (a read-only usage tray app) *send* to Claude on a schedule:

1. **Scheduled messages** — fire a chosen prompt daily/weekly, optionally
   skipping when a 5h session window is already active.
2. **5h-window priming** — send a tiny Haiku message at chosen anchor times so a
   fresh 5h *session* window starts early, letting the user fit **3 windows/day
   instead of 2**. Draggable 24h timeline UI to line the windows up with the day.

Future (design for extensibility, not building now): auto-**drain** the last
bits of a 5h/weekly window before it resets.

## Decisions (locked)

- **Send via the Claude Code CLI**: `claude -p "<msg>" --model <model>` — not the
  `/v1/messages` API (avoids subscription-OAuth gating). Binary confirmed at
  `C:\Users\camer\.local\bin\claude` v2.1.204. Run in `temp_dir()` with
  `--setting-sources ""` and `--permission-mode default`.
- **"Only if session not active"** = skip when a 5h session window is already
  active per polled usage. No process detection.
- **Default prime model**: Haiku (`claude-haiku-4-5`), configurable.
- **Draggable anchor** on the timeline (click-drag), not a read-only preview.
- **Slot spacing = 5h + a few seconds slack** (default 15s) to avoid boundary
  aliasing — a prime lands just *after* the previous window has surely reset, not
  exactly on the edge where jitter could drop it into the old window (wasted) or
  race the reset.
- **Verify the session started** after every prime: re-poll usage and confirm the
  session window went active (resets_at ≈ now+5h). Record verified/unverified in
  the send-log so a silently-failed prime is visible.

## Files

- `src-tauri/src/config.rs` — extend `Config` (+`ScheduledMessage`,
  `PrimingConfig`); all `#[serde(default)]` so old `config.json` upgrades.
- `src-tauri/src/sender.rs` (new) — spawn the CLI, `SendOutcome`, post-send verify.
- `src-tauri/src/schedule.rs` (new) — persisted `ScheduleState`
  (`schedule_state.json`) + unit-testable `due()` evaluator.
- `src-tauri/src/lib.rs` — `Snapshot.session_active`; scheduler async task;
  commands `send_message_now`, `get_send_log`; register in `generate_handler!`.
- `src-tauri/src/metrics.rs` / `usage.rs` — `session_active` derived from the
  `session` `Limit` (`is_active`, or future `resets_at` with `percent > 0`).
- `src/lib/usage.ts` — new types + `invoke` wrappers.
- `src/lib/SettingsPanel.svelte` — two new sections + draggable timeline SVG.
- `README.md` — new section + config keys + ToS caveat.

## Slot math (priming)

`slot(k) = anchor_local + k * (5h + slack)` for `k in 0..windows_per_day`, drop
slots past optional `end_of_day`. `slack` default 15s. A prime fires for the slot
that just came due (within a grace window) only if **not** session_active and not
already fired today (persisted key `YYYY-MM-DD#k`).

## Verify-after-prime

After a successful `claude -p`, run a bounded re-poll (a couple tries, ~10–20s
apart): fetch usage, check the session window is active with `resets_at` ~5h out.
Set `SendOutcome.verified = Some(true/false)` and emit `send-log-updated`. A
prime that sent but didn't flip the window to active shows a warning row.

## Scheduler wiring

Second async task in `setup()` beside the poll loop; ticks ~30s; reads live
`Config` + latest `session_active`; calls `schedule::due(now_local, cfg, state,
session_active)`; runs jobs through `sender::send`; persists `ScheduleState`.
Grace window + persisted last-fired prevents wake-from-sleep floods / double
fires; stale slots are marked fired without sending.

## UI

Two sections below History, matching `.field`/`.row`/`.hint` styling, no `title=`
tooltips:
- **Scheduled messages** — add/remove list; per row: enable, time, weekday chips,
  model select, message textarea, "skip if 5h window active", **Send now**.
- **Window priming** — enable, model, windows/day, optional end-of-day, and a
  **draggable 24h SVG timeline**: drag the anchor block; `windows_per_day` 5h
  blocks re-lay live, each labeled start–end; current session window overlaid.
  Caption explains the 3-windows/day idea + that empty slots auto-prime with a
  tiny Haiku message only when no window is already running.

## Verification

1. `cd src-tauri && cargo test --lib` — unit tests for `schedule::due` (time/
   grace/weekday/session-skip; prime slot math incl. slack, windows_per_day,
   end_of_day).
2. **Send now** (Haiku) from Settings → send-log row OK, then the app's usage
   poll shows the 5h session window active (proves the prime feeds the window).
3. Anchor a couple minutes out, no active window → one prime fires, verify step
   flips session active, next slot skipped while covered.
4. `npm run check` + `npm run tauri dev` — add a scheduled message, confirm it
   persists across restart and `schedule_state.json` records last-fired.
5. README updated.

## Progress log

- [x] Explore + design + approval.
- [x] config.rs types (`ScheduledMessage`, `PrimingConfig`, 3 Config fields).
- [x] sender.rs (`claude -p`, binary resolver w/ PATHEXT, JSON summary).
- [x] schedule.rs + 7 unit tests (all passing).
- [x] lib.rs wiring: `Snapshot.session_active`, scheduler task (30s tick),
      `send_message_now` + `get_send_log` commands, verify-after-prime.
- [x] Cargo.toml: added tokio `process` feature.
- [x] usage.ts types + wrappers.
- [x] SettingsPanel.svelte + PrimingTimeline.svelte (draggable anchor, blue/green/
      now overlays), Recent sends log.
- [x] README updated (feature section, architecture, config keys, ToS caveat).
- [x] Verified: `cargo test --lib` 27/27 pass; `npm run check` clean; real
      `claude -p` haiku send succeeded headlessly and moved the 5h window 42%→43%.

## Findings / gotchas

- A 5h window is anchored to the *first* message; priming exploits this. Verified
  a haiku `claude -p` counts toward the session window (42%→43%).
- The `claude -p` JSON carries `result` + `total_cost_usd` + `usage` — parsed by
  `sender::summarize_stdout`. Cost is dominated by cache-creation of the CLI's
  system prompt (~$0.017), not the trivial reply.
- Usage endpoint 429s under rapid re-polling (5-min retry-after floor) — don't
  hammer it when verifying; the app's polite 2-min poll is fine.
- "Starts a window from idle" is only observable after the current window resets;
  couldn't fully demo it because a window was already active at test time.

## Not done / future

- Auto-**drain** the last bits of a 5h/weekly window before reset (designed-for
  but not built). The `sender` + `schedule` split makes this a new job source.
- Interactive dev-app (`npm run tauri dev`) walkthrough of the new Settings UI
  was not run in this session (headless env); svelte-check is clean.

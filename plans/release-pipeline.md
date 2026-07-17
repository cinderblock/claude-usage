# Release Pipeline — scrub, publish, CI, self-update

## Goal

Take the repo public: rewrite git history to remove personal data from `plans/`,
bring the plan docs current, publish to GitHub, add CI builds, and wire up Tauri
self-updates from GitHub Releases.

## Environment / context

- Repo: local-only (no remote yet) at the project root; 17 commits, all authored
  `Cameron Tacklind <cameron@tacklind.com>` (public identity — intentionally kept).
- GitHub account: `cinderblock` (gh CLI authenticated, ssh protocol).
- App: Tauri v2 + SvelteKit, `productName` "Claude Usage", identifier
  `com.cinderblock.claude-usage`, version 0.1.0. npm (package-lock.json), not bun.
- Windows is the only tested platform.

## Decisions already made (don't re-ask)

- **Scrub scope:** identity + account configuration (home-dir paths / username,
  plan tier tied to "this account", real billing dollar figures). Observed
  utilization percentages in incident forensics stay — they're diagnostic record,
  not identifying. Commit author name/email stays (public git identity).
- **Rewrite mechanics:** rewrite in a temp clone with `git filter-repo
  --replace-text`, then swap refs in the main repo via `git reset --mixed` —
  never `--hard`; the working tree has another session's uncommitted chart WIP
  (`markDays` day-boundary feature in `UsageChart.svelte` + `+page.svelte`) that
  must survive untouched and uncommitted.
- **Publish:** public repo `cinderblock/claude-usage`.
- **Releases:** Windows-only artifacts for now (only tested platform); CI checks
  run cross-platform-cheap (frontend on ubuntu, Rust tests on windows).
- **Self-update:** `tauri-plugin-updater` v2, endpoint = GitHub Releases
  `latest.json`; signing keypair generated locally, private key + password stored
  as GitHub Actions secrets, key file kept OUT of the repo (`~/.tauri/`).

## Plan / steps

1. [x] Enumerate personal strings across all history (all in
   `plans/claude-usage-watcher.md`).
2. [x] Safety snapshot (`git stash create/store`), backup branch
   (`backup/pre-scrub`), temp-clone rewrite with filter-repo, verified, `main`
   swapped via `git reset --mixed`.
3. [x] Working-copy scrub applied (plans + usage.rs) +
   `claude-usage-watcher.md` brought current.
4. [x] Published: https://github.com/cinderblock/claude-usage (public; only
   `main` pushed — `backup/pre-scrub` stays local).
5. [x] Updater: tauri-plugin-updater wired in `lib.rs` (check 20s after
   startup + daily + "Check for updates" tray item), `createUpdaterArtifacts`,
   pubkey + GitHub `latest.json` endpoint in `tauri.conf.json`; key at
   `~/.tauri/claude-usage.key` (+ `.password` beside it), both set as repo
   Actions secrets.
6. [x] CI + release workflows added; local `npm run check` + `cargo test`
   (14/14) green first.
7. [x] README: install / self-update / CI + release docs; chart wording fixed.
8. [x] v0.1.0 released (2026-07-16): CI green (after adding `@types/node` —
   CI had no Node types where local hoisting hid the gap), release workflow
   green, assets = NSIS exe + MSI + `.sig`s + `latest.json`;
   `releases/latest/download/latest.json` verified serving per-platform
   signatures matching the configured endpoint.

## Remaining / follow-ups

- The install-and-restart update path can only be exercised end-to-end by the
  NEXT release: bump versions (`tauri.conf.json`, `Cargo.toml`,
  `package.json`), tag `v0.1.1`, and a running v0.1.0 should toast + restart.
- The currently-running local instance predates the updater — install v0.1.0
  from the release once to get onto the update train.
- Local-only leftovers, keep until confident: branch `backup/pre-scrub` and
  stash "pre-history-rewrite snapshot" (unscrubbed history — never push).

## Findings / gotchas

- The scrub also had to cover `src-tauri/src/usage.rs`: the serde test fixture
  embedded the real captured billing response (`monthly_limit: 5000,
  used_credits: 1202`) and the test asserted on it — fixture + assertion
  replaced together (placeholders 10000/2500/25.0) so tests still pass.
- First verification attempt contaminated the temp clone: fetching the
  original repo into it for a diff put the unscrubbed history back in
  `refs/remotes`, which a second filter-repo pass then processed. Redo rule:
  never fetch the original into the rewrite clone; verify greps in the clone
  first, then fetch the CLEAN history into the original repo and diff there.
- Personal-string variants in history (all in plans/claude-usage-watcher.md):
  home paths (2 forms), `mighty-purring-moth` scratch-plan path, `(this account:
  \`default_claude_max_20x\`)`, "mostly null on this account", billing figures
  `monthly_limit:5000, used_credits:1202, utilization:24.04` (two line-wrappings,
  but the literal avoids the wrap point), `($50.00 cap, $12.02 used)`,
  `"$12.02 / $50.00 · 24%"`.
- `git filter-repo` not installed as a git subcommand; install via
  `uv tool install git-filter-repo` (uv 0.10.4 present).
- filter-repo hard-resets the working tree in a non-bare repo → NEVER run it in
  the main worktree; temp clone + ref swap instead.
- First signing keypair had to be discarded: its random password lived only in
  a shell variable and the file write silently misbound (PowerShell
  `Set-Content -NoNewline <path> <value>` bound the VALUE as the path,
  creating a stray password-named file in the repo root — deleted, never
  committed). Regenerated writing the password file first and verifying;
  pipeline form (`... | Set-Content -NoNewline $file`) binds correctly.
- Local `cargo test` (and CI's) needs the frontend built first:
  `tauri::generate_context!` embeds `../build` at compile time.
- Stale `@ts-expect-error` in `vite.config.js` failed `svelte-check` — the
  `process` global apparently typed now; removed.

## Things not to do

- No `git reset --hard`, `git checkout --`, or stash push/pop in the shared
  worktree; don't stage or commit the chart-WIP files.
- Don't commit the updater private key or its password; `.tauri` key path stays
  outside the repo.
- Don't push until history verification greps come back clean.

## Progress log

- [x] Confirmed repo unpublished (no remote); history scan done.
- [ ] History rewritten + verified.
- [ ] Plans current + committed.
- [ ] Published.
- [ ] Updater working (signed, endpoint live).
- [ ] CI green on tag.

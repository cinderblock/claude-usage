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
4. [ ] `gh repo create cinderblock/claude-usage --public --source . --push`.
   ← current
5. [ ] Updater: plugin deps + init, `createUpdaterArtifacts`, pubkey + endpoint
   in `tauri.conf.json`, startup + tray-menu update check, signing keys,
   `gh secret set`.
6. [ ] CI: `.github/workflows/ci.yml` (check + test on push/PR) and
   `release.yml` (tauri-action on `v*` tags → GitHub Release + `latest.json`).
7. [ ] README: install / update / build / CI sections.
8. [ ] Tag `v0.1.0`, watch the release build go green, verify `latest.json`.

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

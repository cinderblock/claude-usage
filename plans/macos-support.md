# macOS support

## Goal

Make the tray app work nicely on macOS: read Claude Code's OAuth token from
where it actually lives on macOS (the login Keychain, not
`~/.claude/.credentials.json`), behave like a proper menu-bar app (no Dock
icon), and build/test/release macOS artifacts in CI so the in-app updater
serves Macs too.

## Environment / context

- Developed on Windows — **no Mac available in this session**, so macOS code
  compiles here only syntactically (cfg-gated); type-checking happens in the
  new `macos-latest` CI job, and real behavior needs a hand test on a Mac.
- Repo: `C:\Users\camer\git\claude-usage`, branch `main`, direct-to-main
  commit style.
- Tauri 2; plugins already in use: positioner (TrayCenter), autostart (already
  `MacosLauncher::LaunchAgent`), notification, updater, single-instance, log.

## Decisions already made (don't re-ask)

- **Keychain via `security-framework` crate**, not shelling out to
  `/usr/bin/security` — native API, and the token never appears in `ps`
  output. macOS-only dependency, cfg-gated.
- **Read order on macOS: Keychain first, file fallback.** Claude Code on macOS
  stores the blob in the login Keychain under service
  `Claude Code-credentials`, account = `$USER`, value = the same JSON shape as
  `.credentials.json` (`{"claudeAiOauth": {...}}`). Some setups still use the
  file, hence the fallback.
- **Save mirrors load**: refreshed tokens go back to the Keychain if the item
  exists, else to the file — so Claude Code stays in sync either way. Unknown
  top-level keys in the blob are preserved in both paths.
- **`ActivationPolicy::Accessory`** — menu-bar app, no Dock icon, no Cmd-Tab
  entry. Windows (settings/history) still open normally.
- **Release builds `--target universal-apple-darwin`** (one .dmg for both
  Apple Silicon + Intel) via a build matrix in `release.yml`; tauri-action
  merges each matrix job's platforms into the one `latest.json`.
- **No Apple Developer cert** — Tauri ad-hoc signs the .app. First launch
  needs `xattr -cr` (documented in README). The updater's own downloads don't
  get quarantined, so updates work normally after that.
- Tray icon stays the colored splat (status color IS the signal) — not a
  macOS template icon.

## Plan / steps

1. [x] Survey Windows-isms (credentials, tray, updater, CI, docs).
2. [x] `Cargo.toml`: add macOS-only `security-framework` dep.
3. [x] `credentials.rs`: Keychain read/write with file fallback.
4. [x] `lib.rs`: Accessory activation policy on macOS.
5. [x] `ci.yml`: run the rust job on windows + macos.
6. [x] `release.yml`: matrix (windows, macos universal), mac rust targets.
7. [x] README: macOS install (Gatekeeper), Keychain note, log path, CI text.
8. [x] Verify on Windows (`cargo test`, 20/20), commit `68611d2`.
9. [x] Pushed → CI run watching `macos-latest` type-check.
10. [ ] Hand-test on a real Mac (see checklist below).

## Findings / gotchas

- The usage-history-window work was uncommitted in the shared tree when this
  session started; the peer session committed it (`ab0baa9`) mid-session.
  Tree was clean before my edits — no hunk-splitting needed after all.
- Keychain service name `Claude Code-credentials` / account `$USER` — matches
  Claude Code's own storage. **Verify on a real Mac** (`security
  find-generic-password -s "Claude Code-credentials"`); if the service name
  ever changes upstream, load() falls back to the file and errors clearly.
- Reading another app's Keychain item triggers a user consent prompt; "Always
  Allow" persists. Same for the first write-back. Documented in README.
- `errSecItemNotFound = -25300` is the one Keychain error that means "fall
  back to the file"; everything else (e.g. user denied the prompt) surfaces
  as a poll error in the UI.
- cfg'd-out code is parsed but NOT type-checked on Windows — a wrong API name
  in the keychain module would only fail in the macOS CI job.
- Repo is NOT rustfmt-formatted — don't run `cargo fmt`; match style by hand.

## macOS hand-test checklist (needs real hardware)

- [ ] Keychain read works + prompt appears once ("Always Allow").
- [ ] Token refresh writes back; `claude` CLI still logs in fine afterward.
- [ ] No Dock icon; tray icon legible on light + dark menu bar (32px colored
      splat — if it looks soft, render at 44px or add @2x handling).
- [ ] Popup opens under the menu-bar icon (positioner TrayCenter), hides on blur.
- [ ] Settings/History windows open, focus, hide-on-close.
- [ ] Notifications appear (bundled app; permission prompt on first alert).
- [ ] Autostart (LaunchAgent) toggle works.
- [ ] Updater: install a tagged build, tag a newer one, watch it self-update.

## Things not to do

- Don't run `cargo fmt` (repo isn't rustfmt-clean).
- Don't shell out to `security` with the token on the command line (visible
  in `ps`).
- Don't gate the billing/file logic behind macOS — the file path must keep
  working there too (Claude Code setups vary).

## Progress log

- [x] Surveyed code + CI; identified credentials/Keychain as the only real
      platform gap; the rest is polish (activation policy) + build targets.
- [x] Implemented + committed (`68611d2`), pushed.
- [x] CI green on macos-latest — Keychain code type-checks, all tests pass.
- [ ] Real-Mac verification (hand-test checklist above; next release tag will
      also produce the first universal .dmg to test with).

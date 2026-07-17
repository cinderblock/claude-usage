//! Sends a message to Claude by invoking the local Claude Code CLI in headless
//! print mode (`claude -p`).
//!
//! We deliberately shell out rather than POST to `/v1/messages`: the
//! subscription OAuth token is gated to Claude Code's own identity, and the CLI
//! already handles that auth. Headless usage counts toward the same 5-hour and
//! weekly pools — which is exactly what window "priming" relies on.

use serde::Serialize;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;

/// Hard cap on a single send. A trivial prime returns in seconds; this only
/// guards against a wedged process.
const SEND_TIMEOUT_SECS: u64 = 120;

/// Result of one send, kept in a small ring in `AppState` for the UI.
#[derive(Debug, Clone, Serialize)]
pub struct SendOutcome {
    /// Epoch ms when the send finished.
    pub ts: i64,
    pub ok: bool,
    pub model: String,
    /// Why we sent: `"manual"`, a scheduled-message id, or `"prime#k"`.
    pub source: String,
    /// Short human detail or error message.
    pub detail: String,
    /// Post-send check that a 5h session window went active. `None` when not a
    /// prime, or not checked. `Some(false)` means the send returned OK but the
    /// window didn't start — a silent failure worth surfacing.
    pub verified: Option<bool>,
}

/// Locate a usable `claude` executable: the configured path if set and present,
/// else `claude` on `PATH` (honoring Windows `PATHEXT`), else
/// `~/.local/bin/claude*` (the native installer's location).
pub fn resolve_binary(configured: &str) -> Option<PathBuf> {
    if !configured.is_empty() {
        let p = PathBuf::from(configured);
        if p.is_file() {
            return Some(p);
        }
    }
    if let Some(p) = which_on_path("claude") {
        return Some(p);
    }
    if let Some(home) = dirs::home_dir() {
        for name in ["claude.exe", "claude.cmd", "claude"] {
            let p = home.join(".local").join("bin").join(name);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

/// Find `name` on `PATH`, trying `PATHEXT` extensions on Windows.
fn which_on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let bare = dir.join(name);
        if bare.is_file() {
            return Some(bare);
        }
        if cfg!(windows) {
            let exts = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
            for ext in exts.split(';') {
                let cand = dir.join(format!("{name}{ext}"));
                if cand.is_file() {
                    return Some(cand);
                }
            }
        }
    }
    None
}

/// Run `claude -p <message> --model <model>` headlessly and return the outcome.
///
/// Runs in the OS temp dir with settings layering disabled so no project/user
/// `CLAUDE.md` or settings leak into the prime (auth lives in
/// `.credentials.json`, separate from settings, so it still works). The `ts` is
/// set by the caller-relevant clock; `verified` is left `None` here and filled
/// in by the caller for primes.
pub async fn send(bin: &std::path::Path, message: &str, model: &str, source: &str) -> SendOutcome {
    let now_ms = chrono::Utc::now().timestamp_millis();
    let mut cmd = Command::new(bin);
    cmd.arg("-p")
        .arg(message)
        .arg("--model")
        .arg(model)
        .arg("--output-format")
        .arg("json")
        // Don't layer any settings files — a prime should be hermetic.
        .arg("--setting-sources")
        .arg("")
        .arg("--permission-mode")
        .arg("default")
        .current_dir(std::env::temp_dir())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let spawned = cmd.spawn();
    let child = match spawned {
        Ok(c) => c,
        Err(e) => {
            return SendOutcome {
                ts: now_ms,
                ok: false,
                model: model.to_string(),
                source: source.to_string(),
                detail: format!("failed to launch {}: {e}", bin.display()),
                verified: None,
            }
        }
    };

    let waited = tokio::time::timeout(
        Duration::from_secs(SEND_TIMEOUT_SECS),
        child.wait_with_output(),
    )
    .await;

    match waited {
        Err(_) => SendOutcome {
            ts: chrono::Utc::now().timestamp_millis(),
            ok: false,
            model: model.to_string(),
            source: source.to_string(),
            detail: format!("timed out after {SEND_TIMEOUT_SECS}s"),
            verified: None,
        },
        Ok(Err(e)) => SendOutcome {
            ts: chrono::Utc::now().timestamp_millis(),
            ok: false,
            model: model.to_string(),
            source: source.to_string(),
            detail: format!("process error: {e}"),
            verified: None,
        },
        Ok(Ok(out)) => {
            let ts = chrono::Utc::now().timestamp_millis();
            if out.status.success() {
                SendOutcome {
                    ts,
                    ok: true,
                    model: model.to_string(),
                    source: source.to_string(),
                    detail: summarize_stdout(&out.stdout),
                    verified: None,
                }
            } else {
                // Prefer stderr for the reason; fall back to stdout.
                let mut detail = String::from_utf8_lossy(&out.stderr).trim().to_string();
                if detail.is_empty() {
                    detail = String::from_utf8_lossy(&out.stdout).trim().to_string();
                }
                let detail: String = detail.chars().take(300).collect();
                let code = out
                    .status
                    .code()
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "signal".into());
                SendOutcome {
                    ts,
                    ok: false,
                    model: model.to_string(),
                    source: source.to_string(),
                    detail: format!(
                        "exit {code}: {}",
                        if detail.is_empty() {
                            "no output".into()
                        } else {
                            detail
                        }
                    ),
                    verified: None,
                }
            }
        }
    }
}

/// Pull a short human summary out of `--output-format json` stdout, falling back
/// to a truncated raw string. The JSON result carries `result` (the model's
/// text) and cost/usage fields; we only surface a hint.
fn summarize_stdout(stdout: &[u8]) -> String {
    let s = String::from_utf8_lossy(stdout);
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(s.trim()) {
        let reply = v
            .get("result")
            .and_then(|r| r.as_str())
            .unwrap_or("")
            .trim();
        let cost = v.get("total_cost_usd").and_then(|c| c.as_f64());
        let reply: String = reply.chars().take(60).collect();
        return match cost {
            Some(c) => format!("ok — reply: {reply:?} (${c:.4})"),
            None => format!("ok — reply: {reply:?}"),
        };
    }
    let raw: String = s.trim().chars().take(120).collect();
    format!("ok — {raw}")
}

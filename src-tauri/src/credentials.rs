//! Reads (and, when needed, refreshes) the OAuth token that Claude Code stores
//! locally — at `~/.claude/.credentials.json` on Windows/Linux, in the login
//! Keychain on macOS (file as fallback). We reuse the same token to call the
//! usage endpoint — no separate login. Refreshed tokens are written back to
//! wherever the blob was found so Claude Code stays in sync.

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Public OAuth client id for "Claude Code" (confirmed as `application.uuid`
/// in the `/api/oauth/profile` response). Used only for refresh_token grants.
pub const CLAUDE_CODE_CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";

const TOKEN_URL: &str = "https://console.anthropic.com/v1/oauth/token";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuth {
    #[serde(rename = "accessToken")]
    pub access_token: String,
    #[serde(rename = "refreshToken")]
    pub refresh_token: String,
    /// Epoch milliseconds when `access_token` expires.
    #[serde(rename = "expiresAt")]
    pub expires_at: i64,
    #[serde(default)]
    pub scopes: Vec<String>,
    #[serde(rename = "subscriptionType", default)]
    pub subscription_type: Option<String>,
    #[serde(rename = "rateLimitTier", default)]
    pub rate_limit_tier: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CredentialsFile {
    #[serde(rename = "claudeAiOauth")]
    claude_ai_oauth: OAuth,
}

impl OAuth {
    /// True once the access token is within `skew_secs` of expiry (or past it).
    pub fn is_expired(&self, skew_secs: i64) -> bool {
        let now_ms = chrono::Utc::now().timestamp_millis();
        now_ms >= self.expires_at - skew_secs * 1000
    }
}

/// `~/.claude/.credentials.json`
pub fn credentials_path() -> Result<PathBuf> {
    let home = dirs::home_dir().ok_or_else(|| anyhow!("could not resolve home directory"))?;
    Ok(home.join(".claude").join(".credentials.json"))
}

/// macOS: Claude Code keeps the same JSON blob in the login Keychain instead
/// of on disk. Native Security.framework calls (not the `security` CLI) so
/// the token never appears in `ps` output.
#[cfg(target_os = "macos")]
mod keychain {
    use anyhow::{anyhow, Context, Result};

    /// Keychain service name Claude Code stores its OAuth blob under
    /// (account = the current user's login name).
    pub const SERVICE: &str = "Claude Code-credentials";
    /// `errSecItemNotFound` — the one error that means "no such item, fall
    /// back to the credentials file" rather than a real failure.
    const ERR_SEC_ITEM_NOT_FOUND: i32 = -25300;

    fn account() -> Result<String> {
        std::env::var("USER").context("USER not set — cannot address the Keychain item")
    }

    /// The raw JSON blob from the login Keychain, or `None` if the item
    /// doesn't exist. The first access prompts the user to allow this app to
    /// read Claude Code's item ("Always Allow" persists).
    pub fn read_blob() -> Result<Option<String>> {
        match security_framework::passwords::get_generic_password(SERVICE, &account()?) {
            Ok(bytes) => Ok(Some(
                String::from_utf8(bytes).context("Keychain credentials blob is not UTF-8")?,
            )),
            Err(e) if e.code() == ERR_SEC_ITEM_NOT_FOUND => Ok(None),
            Err(e) => Err(anyhow!(
                "reading Keychain item '{SERVICE}' (denied the Keychain prompt?): {e}"
            )),
        }
    }

    pub fn write_blob(json: &str) -> Result<()> {
        security_framework::passwords::set_generic_password(SERVICE, &account()?, json.as_bytes())
            .map_err(|e| anyhow!("writing Keychain item '{SERVICE}': {e}"))
    }
}

/// The raw credentials JSON from wherever it lives: the Keychain on macOS
/// (file as fallback when the item doesn't exist), the file elsewhere.
fn read_raw() -> Result<String> {
    #[cfg(target_os = "macos")]
    if let Some(blob) = keychain::read_blob()? {
        return Ok(blob);
    }
    let path = credentials_path()?;
    std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))
}

/// Load the current OAuth blob.
pub fn load() -> Result<OAuth> {
    let raw = read_raw()?;
    let parsed: CredentialsFile =
        serde_json::from_str(&raw).context("parsing credentials JSON")?;
    Ok(parsed.claude_ai_oauth)
}

#[derive(Debug, Deserialize)]
struct RefreshResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    /// Seconds until the new access token expires.
    #[serde(default)]
    expires_in: Option<i64>,
}

/// Exchange the refresh token for a fresh access token. Returns the updated
/// `OAuth`. Does NOT write to disk — callers decide whether to persist.
pub async fn refresh(client: &reqwest::Client, current: &OAuth) -> Result<OAuth> {
    let body = serde_json::json!({
        "grant_type": "refresh_token",
        "refresh_token": current.refresh_token,
        "client_id": CLAUDE_CODE_CLIENT_ID,
    });

    let resp = client
        .post(TOKEN_URL)
        .json(&body)
        .send()
        .await
        .context("refresh request failed")?;

    if !resp.status().is_success() {
        let code = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(anyhow!("token refresh returned {code}: {text}"));
    }

    let r: RefreshResponse = resp.json().await.context("parsing refresh response")?;
    let expires_in = r.expires_in.unwrap_or(8 * 3600);
    let mut updated = current.clone();
    updated.access_token = r.access_token;
    if let Some(rt) = r.refresh_token {
        updated.refresh_token = rt;
    }
    updated.expires_at = chrono::Utc::now().timestamp_millis() + expires_in * 1000;
    Ok(updated)
}

/// Write the refreshed tokens back to wherever the blob was found — the
/// Keychain when Claude Code's item exists (macOS), `.credentials.json`
/// otherwise — preserving any other top-level keys. File writes are temp-file
/// + rename so a crash mid-write can't corrupt the file that Claude Code also
/// depends on.
pub fn save(updated: &OAuth) -> Result<()> {
    #[cfg(target_os = "macos")]
    if let Some(existing) = keychain::read_blob()? {
        let mut root: serde_json::Value =
            serde_json::from_str(&existing).unwrap_or_else(|_| serde_json::json!({}));
        root["claudeAiOauth"] = serde_json::to_value(updated)?;
        return keychain::write_blob(&serde_json::to_string(&root)?);
    }

    let path = credentials_path()?;
    // Preserve unknown keys: re-read, replace only claudeAiOauth.
    let mut root: serde_json::Value = match std::fs::read_to_string(&path) {
        Ok(s) => serde_json::from_str(&s).unwrap_or_else(|_| serde_json::json!({})),
        Err(_) => serde_json::json!({}),
    };
    root["claudeAiOauth"] = serde_json::to_value(updated)?;

    let dir = path
        .parent()
        .ok_or_else(|| anyhow!("credentials path has no parent"))?;
    let tmp = dir.join(".credentials.json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(&root)?)
        .with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, &path).context("atomic rename of credentials file")?;
    Ok(())
}

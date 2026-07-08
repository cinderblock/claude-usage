//! Client + models for the Claude subscription usage endpoint.
//!
//! `GET https://api.anthropic.com/api/oauth/usage` with the OAuth bearer token
//! returns the same rate-limit windows the webapp shows.

use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const PROFILE_URL: &str = "https://api.anthropic.com/api/oauth/profile";
const OAUTH_BETA: &str = "oauth-2025-04-20";
const USER_AGENT: &str = "claude-cli/usage-watcher";

/// One rolling limit window (5-hour, 7-day, or per-model scoped weekly).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Limit {
    /// e.g. "session", "weekly_all", "weekly_scoped".
    pub kind: String,
    /// e.g. "session", "weekly".
    #[serde(default)]
    pub group: Option<String>,
    pub percent: f64,
    /// e.g. "normal", "warning".
    #[serde(default)]
    pub severity: Option<String>,
    #[serde(default)]
    pub resets_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub scope: Option<Scope>,
    #[serde(default)]
    pub is_active: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Scope {
    #[serde(default)]
    pub model: Option<ScopeModel>,
    #[serde(default)]
    pub surface: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScopeModel {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub display_name: Option<String>,
}

/// The simple top-level windows (also represented inside `limits`, but these are
/// convenient and always present).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SimpleWindow {
    #[serde(default)]
    pub utilization: f64,
    #[serde(default)]
    pub resets_at: Option<DateTime<Utc>>,
}

/// Usage-based ("extra") billing pool — a monthly credit budget that kicks in
/// once plan limits are hit. Amounts are integer minor units (e.g. cents),
/// scaled by `decimal_places`. The endpoint gives no reset timestamp for it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExtraUsage {
    #[serde(default)]
    pub is_enabled: bool,
    /// Monthly cap in minor units (e.g. 2500 = $25.00 at 2 decimal places).
    #[serde(default)]
    pub monthly_limit: Option<f64>,
    /// Spent so far, minor units.
    #[serde(default)]
    pub used_credits: Option<f64>,
    /// Percent of the monthly cap used (may carry decimals).
    #[serde(default)]
    pub utilization: f64,
    #[serde(default)]
    pub currency: Option<String>,
    #[serde(default)]
    pub decimal_places: Option<u32>,
    #[serde(default)]
    pub disabled_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageResponse {
    #[serde(default)]
    pub five_hour: SimpleWindow,
    #[serde(default)]
    pub seven_day: SimpleWindow,
    #[serde(default)]
    pub extra_usage: Option<ExtraUsage>,
    #[serde(default)]
    pub limits: Vec<Limit>,
}

impl Scope {
    /// Stable string key for a scope, used for history rows + de-dup.
    pub fn key(scope: &Option<Scope>) -> String {
        match scope {
            None => "all".to_string(),
            Some(s) => {
                let model = s
                    .model
                    .as_ref()
                    .and_then(|m| m.display_name.clone().or_else(|| m.id.clone()))
                    .unwrap_or_else(|| "all".to_string());
                let surface = s.surface.clone().unwrap_or_default();
                if surface.is_empty() {
                    model
                } else {
                    format!("{model}/{surface}")
                }
            }
        }
    }

    /// Human label for display.
    pub fn label(scope: &Option<Scope>) -> Option<String> {
        scope.as_ref().and_then(|s| {
            s.model
                .as_ref()
                .and_then(|m| m.display_name.clone().or_else(|| m.id.clone()))
        })
    }
}

pub fn build_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .build()
        .context("building HTTP client")
}

async fn authed_get(client: &reqwest::Client, url: &str, token: &str) -> Result<reqwest::Response> {
    let resp = client
        .get(url)
        .bearer_auth(token)
        .header("anthropic-beta", OAUTH_BETA)
        .send()
        .await
        .with_context(|| format!("GET {url}"))?;
    Ok(resp)
}

/// The server told us to slow down (HTTP 429 on the usage endpoint). Carried
/// as a typed error so the poll loop can back off instead of hammering.
#[derive(Debug, Clone)]
pub struct RateLimited {
    /// Seconds from the Retry-After header, when the server provides one.
    pub retry_after_secs: Option<u64>,
}

impl std::fmt::Display for RateLimited {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "usage endpoint rate-limited us (429)")
    }
}

impl std::error::Error for RateLimited {}

/// Fetch usage. Returns `Ok(None)` on 401 (token rejected) so the caller can
/// decide to refresh; 429 is a typed `RateLimited` error; other non-2xx
/// statuses are hard errors.
pub async fn fetch_usage(
    client: &reqwest::Client,
    token: &str,
) -> Result<Option<UsageResponse>> {
    let resp = authed_get(client, USAGE_URL, token).await?;
    if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
        return Ok(None);
    }
    if resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
        let retry_after_secs = resp
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<u64>().ok());
        return Err(anyhow::Error::new(RateLimited { retry_after_secs }));
    }
    if !resp.status().is_success() {
        let code = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(anyhow!("usage endpoint returned {code}: {text}"));
    }
    let parsed = resp.json::<UsageResponse>().await.context("parsing usage JSON")?;
    Ok(Some(parsed))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Profile {
    #[serde(default)]
    pub account: serde_json::Value,
    #[serde(default)]
    pub organization: serde_json::Value,
}

pub async fn fetch_profile(client: &reqwest::Client, token: &str) -> Result<Option<Profile>> {
    let resp = authed_get(client, PROFILE_URL, token).await?;
    if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
        return Ok(None);
    }
    if !resp.status().is_success() {
        return Ok(None);
    }
    Ok(Some(resp.json::<Profile>().await.context("parsing profile JSON")?))
}

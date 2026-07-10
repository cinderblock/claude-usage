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

/// Accept an explicit JSON `null` as the type's default. `#[serde(default)]`
/// alone only covers a *missing* key — when the backend is degraded it nulls
/// out fields it normally populates, and one nulled scalar would otherwise
/// fail the entire snapshot parse.
fn null_default<'de, D, T>(d: D) -> std::result::Result<T, D::Error>
where
    T: Deserialize<'de> + Default,
    D: serde::Deserializer<'de>,
{
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}

/// Parse each limit entry independently, dropping ones that don't conform
/// (e.g. `"percent": null` during a degraded period) instead of rejecting the
/// whole response. A dropped entry means "no data for that window this poll" —
/// far better than either fabricating a 0% or losing every other window too.
fn lenient_limits<'de, D>(d: D) -> std::result::Result<Vec<Limit>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = Option::<Vec<serde_json::Value>>::deserialize(d)?.unwrap_or_default();
    Ok(raw
        .into_iter()
        .filter_map(|v| serde_json::from_value(v).ok())
        .collect())
}

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
    #[serde(default, deserialize_with = "null_default")]
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
    #[serde(default, deserialize_with = "null_default")]
    pub utilization: f64,
    #[serde(default)]
    pub resets_at: Option<DateTime<Utc>>,
}

/// Usage-based ("extra") billing pool — a monthly credit budget that kicks in
/// once plan limits are hit. Amounts are integer minor units (e.g. cents),
/// scaled by `decimal_places`. The endpoint gives no reset timestamp for it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExtraUsage {
    #[serde(default, deserialize_with = "null_default")]
    pub is_enabled: bool,
    /// Monthly cap in minor units (e.g. 2500 = $25.00 at 2 decimal places).
    #[serde(default)]
    pub monthly_limit: Option<f64>,
    /// Spent so far, minor units.
    #[serde(default)]
    pub used_credits: Option<f64>,
    /// Percent of the monthly cap used (may carry decimals).
    #[serde(default, deserialize_with = "null_default")]
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
    #[serde(default, deserialize_with = "null_default")]
    pub five_hour: SimpleWindow,
    #[serde(default, deserialize_with = "null_default")]
    pub seven_day: SimpleWindow,
    #[serde(default)]
    pub extra_usage: Option<ExtraUsage>,
    #[serde(default, deserialize_with = "lenient_limits")]
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
    let text = resp.text().await.context("reading usage response body")?;
    match serde_json::from_str::<UsageResponse>(&text) {
        Ok(parsed) => Ok(Some(parsed)),
        Err(e) => {
            // Include the start of the body: without it, an intermittent
            // "parsing usage JSON" error is undiagnosable after the fact.
            // The body is usage data, never credentials, so this is safe to
            // surface in the UI (short) and the log (longer).
            let log_snippet: String = text.chars().take(1000).collect();
            log::warn!("usage JSON parse failed ({e}); body[..1000]: {log_snippet}");
            let snippet: String = text.chars().take(200).collect();
            let snippet = if snippet.is_empty() { "<empty body>".to_string() } else { snippet };
            Err(anyhow!("parsing usage JSON ({e}) — body starts: {snippet}"))
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The real response shape captured live on 2026-07-08 (values truncated).
    #[test]
    fn parses_real_response_shape() {
        let body = r#"{
            "five_hour": {"utilization": 64, "resets_at": "2026-07-08T22:29:59.537788+00:00", "limit_dollars": null, "used_dollars": null},
            "seven_day": {"utilization": 100, "resets_at": "2026-07-10T09:59:59.537811+00:00"},
            "seven_day_opus": null,
            "extra_usage": {"is_enabled": true, "monthly_limit": 10000, "used_credits": 2500, "utilization": 25.0, "currency": "USD", "decimal_places": 2, "disabled_reason": null, "daily": null, "weekly": null},
            "limits": [
                {"kind": "session", "group": "session", "percent": 64, "severity": "normal", "resets_at": "2026-07-08T22:29:59.537788+00:00", "scope": null, "is_active": false},
                {"kind": "weekly_scoped", "group": "weekly", "percent": 97, "severity": "critical", "resets_at": "2026-07-10T09:59:59.538197+00:00", "scope": {"model": {"id": null, "display_name": "Fable"}, "surface": null}, "is_active": false}
            ],
            "spend": {"used": {"amount_minor": 2500}, "can_toggle": false}
        }"#;
        let u: UsageResponse = serde_json::from_str(body).unwrap();
        assert_eq!(u.limits.len(), 2);
        assert_eq!(u.five_hour.utilization, 64.0);
        let eu = u.extra_usage.unwrap();
        assert!(eu.is_enabled);
        assert_eq!(eu.monthly_limit, Some(10000.0));
    }

    /// Degraded backend: scalars nulled out must not fail the whole parse.
    #[test]
    fn tolerates_nulled_scalars() {
        let body = r#"{
            "five_hour": {"utilization": null, "resets_at": null},
            "seven_day": null,
            "extra_usage": {"is_enabled": null, "monthly_limit": null, "used_credits": null, "utilization": null},
            "limits": [
                {"kind": "session", "percent": 42, "is_active": null}
            ]
        }"#;
        let u: UsageResponse = serde_json::from_str(body).unwrap();
        assert_eq!(u.five_hour.utilization, 0.0);
        assert_eq!(u.seven_day.utilization, 0.0);
        assert!(!u.extra_usage.unwrap().is_enabled);
        assert_eq!(u.limits.len(), 1);
        assert_eq!(u.limits[0].percent, 42.0);
    }

    /// A malformed limit entry is dropped; the rest survive. `limits: null`
    /// yields an empty vec (the caller treats all-dropped as a failed poll).
    #[test]
    fn drops_malformed_limit_entries_keeps_rest() {
        let body = r#"{
            "limits": [
                {"kind": "session", "percent": null},
                {"kind": "weekly_all", "percent": 87, "severity": "warning"},
                "garbage",
                {"no_kind": true}
            ]
        }"#;
        let u: UsageResponse = serde_json::from_str(body).unwrap();
        assert_eq!(u.limits.len(), 1);
        assert_eq!(u.limits[0].kind, "weekly_all");

        let u: UsageResponse = serde_json::from_str(r#"{"limits": null}"#).unwrap();
        assert!(u.limits.is_empty());
    }
}

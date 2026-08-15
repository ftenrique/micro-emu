//! Usage (rate-limit) retrieval for the agents the bridge can report on.
//!
//! Two sources, both read directly by the bridge — no MCP involvement:
//!
//! * Codex: the ChatGPT backend API (`/backend-api/wham/usage`), authorized
//!   with the tokens Codex CLI persists in `~/.codex/auth.json`. Returns
//!   5-hour and weekly windows with `used_percent` and reset timestamps.
//! * ZCode: the Z.AI monitor endpoint (`/api/monitor/usage/quota/limit`),
//!   authorized with the coding-plan API key ZCode persists in
//!   `~/.zcode/v2/config.json` (`builtin:zai-coding-plan` provider). The
//!   `TOKENS_LIMIT` entry is the 5-hour token window and `TIME_LIMIT` is the
//!   weekly request quota; both carry `percentage` (used) and
//!   `nextResetTime` (epoch milliseconds).
//!
//! Snapshots are cached per agent in [`UsageCache`] so the selected agent's
//! data survives switching sources, and stale entries are refreshed on the
//! daemon's usage tick rather than on every render.

use serde_json::Value;
use std::time::{Duration, Instant};

/// How often usage snapshots are refreshed. 5 minutes balances freshness
/// with API load on both providers.
pub(crate) const USAGE_REFRESH_INTERVAL: Duration = Duration::from_secs(300);

/// Agent whose usage limits are reported on the strip. Codex is the default
/// because it is the bridge's primary agent; ZCode is opt-in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UsageAgent {
    Codex,
    ZCode,
}

impl UsageAgent {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "codex" => Some(Self::Codex),
            "zcode" => Some(Self::ZCode),
            _ => None,
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::ZCode => "zcode",
        }
    }

    /// Maps a routing agent to its usage counterpart. Hermes reports no
    /// usage limits, so it has none.
    pub(crate) fn from_agent(agent: crate::routing::AgentId) -> Option<Self> {
        match agent {
            crate::routing::AgentId::Codex => Some(Self::Codex),
            crate::routing::AgentId::ZCode => Some(Self::ZCode),
            crate::routing::AgentId::Hermes => None,
        }
    }
}

/// Remaining-percentage and reset-time snapshot shared by both agents.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct UsageSnapshot {
    pub five_hour_remaining: Option<u8>,
    pub weekly_remaining: Option<u8>,
    pub five_hour_reset_at: Option<u64>,
    pub weekly_reset_at: Option<u64>,
}

impl UsageSnapshot {
    /// Wire form embedded in the display context render payload.
    pub(crate) fn to_value(self) -> Value {
        serde_json::json!({
            "five_hour_remaining": self.five_hour_remaining,
            "weekly_remaining": self.weekly_remaining,
            "five_hour_reset_at": self.five_hour_reset_at,
            "weekly_reset_at": self.weekly_reset_at,
        })
    }
}

/// Per-agent cache of the last fetched snapshot.
#[derive(Debug, Default)]
pub struct UsageCache {
    codex: Option<UsageSnapshot>,
    zcode: Option<UsageSnapshot>,
    codex_refreshed_at: Option<Instant>,
    zcode_refreshed_at: Option<Instant>,
}

impl UsageCache {
    pub(crate) fn snapshot(&self, agent: UsageAgent) -> Option<UsageSnapshot> {
        match agent {
            UsageAgent::Codex => self.codex,
            UsageAgent::ZCode => self.zcode,
        }
    }

    /// Serializes every fetched snapshot by agent name so plugin-side
    /// displays can render codex and zcode usage simultaneously. Agents
    /// that were never fetched are omitted.
    pub(crate) fn usage_map(&self) -> Value {
        let mut map = serde_json::Map::new();
        for agent in [UsageAgent::Codex, UsageAgent::ZCode] {
            if let Some(snapshot) = self.snapshot(agent) {
                map.insert(agent.as_str().to_owned(), snapshot.to_value());
            }
        }
        Value::Object(map)
    }

    /// True when the agent's snapshot was refreshed within `max_age`.
    pub(crate) fn refreshed_within(&self, agent: UsageAgent, max_age: Duration) -> bool {
        let refreshed_at = match agent {
            UsageAgent::Codex => self.codex_refreshed_at,
            UsageAgent::ZCode => self.zcode_refreshed_at,
        };
        refreshed_at.is_some_and(|at| at.elapsed() < max_age)
    }

    /// Stores one agent's freshly fetched snapshot. `refreshed_at` moves
    /// forward even on failure so a broken endpoint is not retried on every
    /// event. Returns true when the stored snapshot changed.
    pub(crate) fn store(&mut self, agent: UsageAgent, snapshot: UsageSnapshot) -> bool {
        let changed = self.snapshot(agent) != Some(snapshot);
        match agent {
            UsageAgent::Codex => {
                self.codex = Some(snapshot);
                self.codex_refreshed_at = Some(Instant::now());
            }
            UsageAgent::ZCode => {
                self.zcode = Some(snapshot);
                self.zcode_refreshed_at = Some(Instant::now());
            }
        }
        changed
    }
}

/// Fetches one agent's snapshot from its provider.
pub(crate) fn fetch_usage(agent: UsageAgent) -> UsageSnapshot {
    match agent {
        UsageAgent::Codex => fetch_codex_usage(),
        UsageAgent::ZCode => fetch_zcode_usage(),
    }
}

fn home_dir() -> String {
    std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_default()
}

/// Fetches live Codex usage (5-hour and weekly remaining percentages) from
/// the ChatGPT backend API, using the auth tokens from `~/.codex/auth.json`.
pub(crate) fn fetch_codex_usage() -> UsageSnapshot {
    let unavailable = UsageSnapshot::default();
    let auth_path = std::path::Path::new(&home_dir())
        .join(".codex")
        .join("auth.json");
    let Ok(auth_content) = std::fs::read_to_string(&auth_path) else {
        return unavailable;
    };
    let Ok(auth) = serde_json::from_str::<Value>(&auth_content) else {
        return unavailable;
    };
    let tokens = match auth.get("tokens") {
        Some(t) => t,
        None => return unavailable,
    };
    let access_token = tokens.get("access_token").and_then(Value::as_str);
    let account_id = tokens.get("account_id").and_then(Value::as_str);
    let (Some(access_token), Some(account_id)) = (access_token, account_id) else {
        return unavailable;
    };

    let response = match ureq::get("https://chatgpt.com/backend-api/wham/usage")
        .set("Authorization", &format!("Bearer {access_token}"))
        .set("ChatGPT-Account-Id", account_id)
        .set("User-Agent", "codex-cli")
        .set("Accept", "application/json")
        .timeout(Duration::from_secs(10))
        .call()
    {
        Ok(response) => response,
        Err(_) => return unavailable,
    };
    let body: Value = match response.into_json() {
        Ok(body) => body,
        Err(_) => return unavailable,
    };
    parse_codex_usage(&body)
}

/// Parses a ChatGPT `wham/usage` payload. The API may return 5-hour and/or
/// weekly windows in either `primary_window` or `secondary_window`.
fn parse_codex_usage(body: &Value) -> UsageSnapshot {
    let unavailable = UsageSnapshot::default();
    let Some(rate_limit) = body.get("rate_limit") else {
        return unavailable;
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let mut usage = UsageSnapshot::default();
    for key in &["primary_window", "secondary_window"] {
        let Some(window) = rate_limit.get(key) else {
            continue;
        };
        let Some(seconds) = window.get("limit_window_seconds").and_then(Value::as_u64) else {
            continue;
        };
        if seconds == 0 {
            continue;
        }
        let Some(used_percent) = window.get("used_percent").and_then(Value::as_f64) else {
            continue;
        };
        let remaining = remaining_from_used(used_percent);
        let reset_at = window.get("reset_at").and_then(Value::as_u64).or_else(|| {
            window
                .get("reset_after_seconds")
                .and_then(Value::as_u64)
                .map(|after| now.saturating_add(after))
        });
        // <= 24 hours = 5-hour window; >= 3 days = weekly window.
        if seconds <= 24 * 60 * 60 && usage.five_hour_remaining.is_none() {
            usage.five_hour_remaining = Some(remaining);
            usage.five_hour_reset_at = reset_at;
        } else if seconds >= 3 * 24 * 60 * 60 && usage.weekly_remaining.is_none() {
            usage.weekly_remaining = Some(remaining);
            usage.weekly_reset_at = reset_at;
        }
    }
    usage
}

/// Reads the Z.AI coding-plan API key from ZCode's provider config.
fn read_zcode_api_key() -> Option<String> {
    let config_path = std::path::Path::new(&home_dir())
        .join(".zcode")
        .join("v2")
        .join("config.json");
    let content = std::fs::read_to_string(&config_path).ok()?;
    let config: Value = serde_json::from_str(&content).ok()?;
    let provider = config.get("provider")?.get("builtin:zai-coding-plan")?;
    if provider.get("enabled").and_then(Value::as_bool) == Some(false) {
        return None;
    }
    let key = provider
        .get("options")?
        .get("apiKey")?
        .as_str()?
        .trim()
        .to_owned();
    (!key.is_empty()).then_some(key)
}

/// Fetches live ZCode usage from the Z.AI monitor endpoint using the
/// coding-plan key stored in ZCode's own config. No ZCode session is
/// required: the key and the quota are account-level.
pub(crate) fn fetch_zcode_usage() -> UsageSnapshot {
    let Some(api_key) = read_zcode_api_key() else {
        return UsageSnapshot::default();
    };
    let response = match ureq::get("https://api.z.ai/api/monitor/usage/quota/limit")
        .set("Authorization", &format!("Bearer {api_key}"))
        .set("User-Agent", "zcode")
        .set("Accept-Language", "en-US,en")
        .set("Accept", "application/json")
        .timeout(Duration::from_secs(10))
        .call()
    {
        Ok(response) => response,
        Err(_) => return UsageSnapshot::default(),
    };
    let body: Value = match response.into_json() {
        Ok(body) => body,
        Err(_) => return UsageSnapshot::default(),
    };
    parse_zcode_quota(&body)
}

/// Parses a Z.AI `monitor/usage/quota/limit` payload: `TOKENS_LIMIT` is the
/// 5-hour token window, `TIME_LIMIT` the weekly request quota.
fn parse_zcode_quota(body: &Value) -> UsageSnapshot {
    let mut usage = UsageSnapshot::default();
    let Some(limits) = body.get("data").and_then(|data| data.get("limits")) else {
        return usage;
    };
    let Some(limits) = limits.as_array() else {
        return usage;
    };
    for limit in limits {
        let kind = limit.get("type").and_then(Value::as_str).unwrap_or("");
        let used = limit.get("percentage").and_then(Value::as_f64);
        let remaining = limit
            .get("remaining")
            .and_then(Value::as_f64)
            .map(|remaining| remaining.clamp(0.0, 100.0) as u8)
            .or_else(|| used.map(remaining_from_used));
        let reset_at = limit
            .get("nextResetTime")
            .and_then(Value::as_u64)
            .map(epoch_ms_to_secs);
        match kind {
            "TOKENS_LIMIT" if usage.five_hour_remaining.is_none() => {
                usage.five_hour_remaining = remaining;
                usage.five_hour_reset_at = reset_at;
            }
            "TIME_LIMIT" if usage.weekly_remaining.is_none() => {
                usage.weekly_remaining = remaining;
                usage.weekly_reset_at = reset_at;
            }
            _ => {}
        }
    }
    usage
}

/// Converts a used percentage into a clamped remaining percentage.
fn remaining_from_used(used_percent: f64) -> u8 {
    (100.0 - used_percent).round().clamp(0.0, 100.0) as u8
}

/// Z.AI timestamps are epoch milliseconds; display contexts use seconds.
fn epoch_ms_to_secs(value: u64) -> u64 {
    if value > 1_000_000_000_000 {
        value / 1000
    } else {
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_usage_agents() {
        assert_eq!(UsageAgent::parse("codex"), Some(UsageAgent::Codex));
        assert_eq!(UsageAgent::parse(" ZCode "), Some(UsageAgent::ZCode));
        assert_eq!(UsageAgent::parse("hermes"), None);
        assert_eq!(UsageAgent::Codex.as_str(), "codex");
    }

    #[test]
    fn parses_codex_windows_in_either_slot() {
        let body = json!({
            "rate_limit": {
                "primary_window": {
                    "limit_window_seconds": 5 * 60 * 60,
                    "used_percent": 37.4,
                    "reset_at": 1_760_001_800
                },
                "secondary_window": {
                    "limit_window_seconds": 7 * 24 * 60 * 60,
                    "used_percent": 12.0,
                    "reset_after_seconds": 400_000
                }
            }
        });
        let usage = parse_codex_usage(&body);
        assert_eq!(usage.five_hour_remaining, Some(63));
        assert_eq!(usage.five_hour_reset_at, Some(1_760_001_800));
        assert_eq!(usage.weekly_remaining, Some(88));
        assert!(usage.weekly_reset_at.is_some());
    }

    #[test]
    fn codex_windows_without_rate_limit_are_unavailable() {
        assert_eq!(parse_codex_usage(&json!({})), UsageSnapshot::default());
        assert_eq!(
            parse_codex_usage(&json!({"rate_limit": {"primary_window": {}}})),
            UsageSnapshot::default()
        );
    }

    #[test]
    fn parses_zcode_quota_payload() {
        // Shape captured from the live endpoint: TOKENS_LIMIT carries only a
        // percentage, TIME_LIMIT adds an explicit remaining count.
        let body = json!({
            "code": 200,
            "success": true,
            "data": {
                "limits": [
                    {
                        "type": "TIME_LIMIT",
                        "unit": 5,
                        "number": 1,
                        "usage": 100,
                        "currentValue": 2,
                        "remaining": 98,
                        "percentage": 2,
                        "nextResetTime": 1_787_506_025_998_u64
                    },
                    {
                        "type": "TOKENS_LIMIT",
                        "unit": 3,
                        "number": 5,
                        "percentage": 38,
                        "nextResetTime": 1_786_726_948_859_u64
                    }
                ],
                "level": "lite"
            }
        });
        let usage = parse_zcode_quota(&body);
        assert_eq!(usage.five_hour_remaining, Some(62));
        assert_eq!(usage.five_hour_reset_at, Some(1_786_726_948));
        assert_eq!(usage.weekly_remaining, Some(98));
        assert_eq!(usage.weekly_reset_at, Some(1_787_506_025));
    }

    #[test]
    fn zcode_weekly_without_remaining_falls_back_to_percentage() {
        let body = json!({
            "data": {"limits": [
                {"type": "TIME_LIMIT", "percentage": 150.0},
                {"type": "TOKENS_LIMIT", "percentage": 100.0}
            ]}
        });
        let usage = parse_zcode_quota(&body);
        assert_eq!(usage.weekly_remaining, Some(0));
        assert_eq!(usage.five_hour_remaining, Some(0));
    }

    #[test]
    fn zcode_payload_without_limits_is_unavailable() {
        assert_eq!(
            parse_zcode_quota(&json!({"data": {}})),
            UsageSnapshot::default()
        );
        assert_eq!(
            parse_zcode_quota(&json!({"code": 200, "data": null})),
            UsageSnapshot::default()
        );
    }

    #[test]
    fn cache_store_updates_snapshot_and_staleness() {
        let mut cache = UsageCache::default();
        assert!(!cache.refreshed_within(UsageAgent::ZCode, USAGE_REFRESH_INTERVAL));
        cache.store(
            UsageAgent::ZCode,
            UsageSnapshot {
                five_hour_remaining: Some(62),
                weekly_remaining: Some(98),
                ..UsageSnapshot::default()
            },
        );
        assert!(cache.refreshed_within(UsageAgent::ZCode, USAGE_REFRESH_INTERVAL));
        assert!(!cache.refreshed_within(UsageAgent::Codex, USAGE_REFRESH_INTERVAL));
        assert!(cache.snapshot(UsageAgent::Codex).is_none());
        assert_eq!(
            cache.snapshot(UsageAgent::ZCode).unwrap().weekly_remaining,
            Some(98)
        );
    }

    #[test]
    fn usage_map_serializes_fetched_agents_only() {
        let mut cache = UsageCache::default();
        assert!(cache.usage_map().as_object().unwrap().is_empty());
        cache.store(
            UsageAgent::ZCode,
            UsageSnapshot {
                five_hour_remaining: Some(62),
                ..UsageSnapshot::default()
            },
        );
        let map = cache.usage_map();
        assert_eq!(map["zcode"]["five_hour_remaining"], 62);
        assert!(map.get("codex").is_none());
    }
}

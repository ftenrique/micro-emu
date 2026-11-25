//! Usage (rate-limit) retrieval for the agents the bridge can report on.
//!
//! Three sources, all read directly by the bridge — no MCP involvement:
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
//! * Claude: the Anthropic OAuth usage endpoint (`/api/oauth/usage`) — the
//!   same source Claude Code's `/usage` screen reads — authorized with the
//!   access token Claude Code persists in `~/.claude/.credentials.json`.
//!   `five_hour` and `seven_day` carry `utilization` (percent used) and an
//!   ISO 8601 `resets_at`. An expired token is refreshed with Claude Code's
//!   own refresh-token grant and written back atomically, so desktop-only
//!   setups (whose sessions never renew that file) keep reporting usage.
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
/// because it is the bridge's primary agent; pressing another agent's task
/// card switches the source to that agent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UsageAgent {
    Codex,
    ZCode,
    Claude,
}

impl UsageAgent {
    pub(crate) const ALL: [Self; 3] = [Self::Codex, Self::ZCode, Self::Claude];

    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "codex" => Some(Self::Codex),
            "zcode" => Some(Self::ZCode),
            "claude" => Some(Self::Claude),
            _ => None,
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::ZCode => "zcode",
            Self::Claude => "claude",
        }
    }

    /// Maps a routing agent to its usage counterpart. Hermes reports no
    /// usage limits to the bridge, and T3 Code delegates to whichever provider
    /// harness backs the pressed thread (resolved via [`Self::from_task`]).
    pub(crate) fn from_agent(agent: crate::routing::AgentId) -> Option<Self> {
        match agent {
            crate::routing::AgentId::Codex => Some(Self::Codex),
            crate::routing::AgentId::ZCode => Some(Self::ZCode),
            crate::routing::AgentId::Claude => Some(Self::Claude),
            crate::routing::AgentId::Hermes | crate::routing::AgentId::T3 => None,
        }
    }

    /// Maps a pressed/selected task to its usage counterpart. For T3 Code,
    /// follows the underlying provider harness (`codex`, `claudeAgent`,
    /// `glm`/Z.ai); providers without a usage endpoint return `None` so the
    /// current usage source stays visible rather than blanking.
    pub(crate) fn from_task(agent: crate::routing::AgentId, provider: Option<&str>) -> Option<Self> {
        if agent != crate::routing::AgentId::T3 {
            return Self::from_agent(agent);
        }
        let lower = provider?.trim().to_ascii_lowercase();
        if lower == "codex" || lower.ends_with("_codex") {
            Some(Self::Codex)
        } else if lower == "claudeagent" || lower == "claude" || lower.contains("claude") {
            Some(Self::Claude)
        } else if lower.contains("glm") || lower.contains("zcode") || lower.contains("zai") {
            Some(Self::ZCode)
        } else {
            None
        }
    }

    fn index(self) -> usize {
        match self {
            Self::Codex => 0,
            Self::ZCode => 1,
            Self::Claude => 2,
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
    /// Indexed by [`UsageAgent::index`].
    snapshots: [Option<UsageSnapshot>; 3],
    refreshed_at: [Option<Instant>; 3],
    pending: Vec<(UsageAgent, std::sync::mpsc::Receiver<UsageSnapshot>)>,
}

impl UsageCache {
    /// Network requests never run on the controller/serial event loop.
    pub(crate) fn request_refresh(&mut self, agent: UsageAgent) {
        if self.pending.iter().any(|(pending, _)| *pending == agent)
            || self.refreshed_within(agent, USAGE_REFRESH_INTERVAL) {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        self.pending.push((agent, rx));
        std::thread::spawn(move || { let _ = tx.send(fetch_usage(agent)); });
    }

    pub(crate) fn poll_ready(&mut self) -> bool {
        let mut changed = false;
        for (agent, rx) in std::mem::take(&mut self.pending) {
            match rx.try_recv() {
                Ok(snapshot) => changed |= self.store(agent, snapshot),
                Err(std::sync::mpsc::TryRecvError::Empty) => self.pending.push((agent, rx)),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    changed |= self.store(agent, UsageSnapshot::default());
                }
            }
        }
        changed
    }

    pub(crate) fn snapshot(&self, agent: UsageAgent) -> Option<UsageSnapshot> {
        self.snapshots[agent.index()]
    }

    /// Serializes every fetched snapshot by agent name so plugin-side
    /// displays can render several agents' usage simultaneously. Agents
    /// that were never fetched are omitted.
    pub(crate) fn usage_map(&self) -> Value {
        let mut map = serde_json::Map::new();
        for agent in UsageAgent::ALL {
            if let Some(snapshot) = self.snapshot(agent) {
                map.insert(agent.as_str().to_owned(), snapshot.to_value());
            }
        }
        Value::Object(map)
    }

    /// True when the agent's snapshot was refreshed within `max_age`.
    pub(crate) fn refreshed_within(&self, agent: UsageAgent, max_age: Duration) -> bool {
        self.refreshed_at[agent.index()].is_some_and(|at| at.elapsed() < max_age)
    }

    /// Stores one agent's freshly fetched snapshot. `refreshed_at` moves
    /// forward even on failure so a broken endpoint is not retried on every
    /// event. Returns true when the stored snapshot changed.
    pub(crate) fn store(&mut self, agent: UsageAgent, snapshot: UsageSnapshot) -> bool {
        let changed = self.snapshot(agent) != Some(snapshot);
        self.snapshots[agent.index()] = Some(snapshot);
        self.refreshed_at[agent.index()] = Some(Instant::now());
        changed
    }
}

/// Fetches one agent's snapshot from its provider.
pub(crate) fn fetch_usage(agent: UsageAgent) -> UsageSnapshot {
    match agent {
        UsageAgent::Codex => fetch_codex_usage(),
        UsageAgent::ZCode => fetch_zcode_usage(),
        UsageAgent::Claude => fetch_claude_usage(),
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

/// Claude Code's config directory: `$CLAUDE_CONFIG_DIR`, else `~/.claude`.
fn claude_config_dir() -> std::path::PathBuf {
    match std::env::var_os("CLAUDE_CONFIG_DIR") {
        Some(dir) if !dir.is_empty() => std::path::PathBuf::from(dir),
        _ => std::path::Path::new(&home_dir()).join(".claude"),
    }
}

/// Claude Code's OAuth client and token endpoint (from its own config).
const CLAUDE_OAUTH_CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
const CLAUDE_OAUTH_TOKEN_URL: &str = "https://platform.claude.com/v1/oauth/token";

fn read_json_file(path: &std::path::Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// Returns a usable Claude Code access token from `.credentials.json`,
/// refreshing it the same way Claude Code does when it has expired. Sessions
/// hosted by the desktop app authenticate through the app and never renew
/// this file, so without the refresh the token goes stale within hours.
fn claude_access_token(now_ms: u64) -> Option<String> {
    let path = claude_config_dir().join(".credentials.json");
    let credentials = read_json_file(&path)?;
    if let Some(token) = claude_access_token_from(&credentials, now_ms) {
        return Some(token);
    }
    refresh_claude_token(&path, &credentials, now_ms)
}

/// Hash of the last refresh token the server rejected, so a revoked login is
/// not retried every refresh tick (the token itself is not kept around).
static REJECTED_REFRESH_TOKEN: std::sync::Mutex<Option<u64>> = std::sync::Mutex::new(None);

fn token_fingerprint(token: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    token.hash(&mut hasher);
    hasher.finish()
}

/// Exchanges the stored refresh token for a new access token and writes the
/// result back so Claude Code (which watches the file) picks it up too.
fn refresh_claude_token(path: &std::path::Path, credentials: &Value, now_ms: u64) -> Option<String> {
    let (refresh_token, body) = claude_refresh_request(credentials)?;
    let fingerprint = token_fingerprint(&refresh_token);
    if *REJECTED_REFRESH_TOKEN.lock().ok()? == Some(fingerprint) {
        return None;
    }
    let response = match ureq::post(CLAUDE_OAUTH_TOKEN_URL)
        .set("Content-Type", "application/json")
        .set("User-Agent", "claude-code")
        .timeout(Duration::from_secs(15))
        .send_json(body)
    {
        Ok(response) => response.into_json::<Value>().ok()?,
        Err(ureq::Error::Status(status, response)) => {
            // Only the OAuth error code and description are logged; error
            // bodies carry no tokens, but nothing else is echoed regardless.
            let reason = response
                .into_json::<Value>()
                .ok()
                .map(|body| {
                    ["error", "error_description"]
                        .iter()
                        .filter_map(|key| body.get(*key).and_then(Value::as_str))
                        .collect::<Vec<_>>()
                        .join(": ")
                })
                .filter(|reason| !reason.is_empty())
                .unwrap_or_else(|| "no reason given".to_owned());
            eprintln!("Claude token refresh rejected (HTTP {status}): {reason}");
            crate::diaglog::log(&format!("claude token refresh rejected: HTTP {status} {reason}"));
            if matches!(status, 400 | 401 | 403) {
                if let Ok(mut rejected) = REJECTED_REFRESH_TOKEN.lock() {
                    *rejected = Some(fingerprint);
                }
            }
            return None;
        }
        Err(error) => {
            eprintln!("Claude token refresh failed: {error}");
            return None;
        }
    };
    // Claude Code may have refreshed while the request was in flight: if the
    // file moved on to a token that works, keep that one untouched.
    let current = read_json_file(path).unwrap_or_else(|| credentials.clone());
    let stored_refresh = current
        .get("claudeAiOauth")
        .and_then(|oauth| oauth.get("refreshToken"))
        .and_then(Value::as_str);
    if stored_refresh != Some(refresh_token.as_str()) {
        if let Some(token) = claude_access_token_from(&current, now_ms) {
            return Some(token);
        }
    }
    let updated = apply_claude_refresh(&current, &response, now_ms)?;
    let token = updated["claudeAiOauth"]["accessToken"].as_str()?.to_owned();
    if let Err(error) = write_credentials_atomically(path, &updated) {
        // The new token still works for this fetch; the next tick retries the
        // write with the (possibly rotated) refresh token it can still read.
        eprintln!("Claude credentials write-back failed: {error}");
    }
    Some(token)
}

/// Builds Claude Code's refresh-token grant: the stored refresh token, its
/// OAuth client id, and the scopes the login was granted.
fn claude_refresh_request(credentials: &Value) -> Option<(String, Value)> {
    let oauth = credentials.get("claudeAiOauth")?;
    let refresh_token = oauth.get("refreshToken")?.as_str()?.trim().to_owned();
    if refresh_token.is_empty() {
        return None;
    }
    let mut body = serde_json::json!({
        "grant_type": "refresh_token",
        "refresh_token": refresh_token,
        "client_id": CLAUDE_OAUTH_CLIENT_ID,
    });
    let scopes: Vec<&str> = oauth
        .get("scopes")
        .and_then(Value::as_array)
        .map(|scopes| scopes.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    if !scopes.is_empty() {
        body["scope"] = Value::from(scopes.join(" "));
    }
    Some((refresh_token, body))
}

/// Merges a token-endpoint response into the credentials, keeping every
/// other field. A response without a new refresh token keeps the old one.
fn apply_claude_refresh(credentials: &Value, response: &Value, now_ms: u64) -> Option<Value> {
    let access_token = response.get("access_token")?.as_str()?.trim();
    let expires_in = response.get("expires_in").and_then(Value::as_u64)?;
    if access_token.is_empty() {
        return None;
    }
    let mut updated = credentials.clone();
    let oauth = updated.get_mut("claudeAiOauth")?.as_object_mut()?;
    oauth.insert("accessToken".to_owned(), Value::from(access_token));
    if let Some(refresh) = response.get("refresh_token").and_then(Value::as_str) {
        if !refresh.trim().is_empty() {
            oauth.insert("refreshToken".to_owned(), Value::from(refresh.trim()));
        }
    }
    oauth.insert(
        "expiresAt".to_owned(),
        Value::from(now_ms.saturating_add(expires_in.saturating_mul(1000))),
    );
    if let Some(scope) = response.get("scope").and_then(Value::as_str) {
        let scopes: Vec<Value> = scope.split_whitespace().map(Value::from).collect();
        if !scopes.is_empty() {
            oauth.insert("scopes".to_owned(), Value::Array(scopes));
        }
    }
    Some(updated)
}

/// Replaces the credentials file in one step (write a sibling temp file,
/// then rename over the original) so a reader never sees a partial file.
fn write_credentials_atomically(path: &std::path::Path, credentials: &Value) -> Result<(), String> {
    let temp = path.with_extension("json.micro-emu-tmp");
    let text = serde_json::to_string(credentials).map_err(|error| error.to_string())?;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| {
        use std::io::Write;
        let mut file = options.open(&temp)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result.map_err(|error| error.to_string())
}

fn claude_access_token_from(credentials: &Value, now_ms: u64) -> Option<String> {
    let oauth = credentials.get("claudeAiOauth")?;
    // One minute of slack so a token is not sent seconds before it expires.
    if oauth
        .get("expiresAt")
        .and_then(Value::as_u64)
        .is_some_and(|expires_at| expires_at <= now_ms.saturating_add(60_000))
    {
        return None;
    }
    let token = oauth.get("accessToken")?.as_str()?.trim();
    (!token.is_empty()).then(|| token.to_owned())
}

/// Fetches live Claude plan usage from the Anthropic OAuth usage endpoint.
pub(crate) fn fetch_claude_usage() -> UsageSnapshot {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0);
    let Some(token) = claude_access_token(now_ms) else {
        return UsageSnapshot::default();
    };
    let response = match ureq::get("https://api.anthropic.com/api/oauth/usage")
        .set("Authorization", &format!("Bearer {token}"))
        .set("anthropic-beta", "oauth-2025-04-20")
        .set("User-Agent", "claude-code")
        .set("Accept", "application/json")
        .timeout(Duration::from_secs(10))
        .call()
    {
        Ok(response) => response,
        Err(_) => return UsageSnapshot::default(),
    };
    match response.into_json::<Value>() {
        Ok(body) => parse_claude_usage(&body),
        Err(_) => UsageSnapshot::default(),
    }
}

/// Parses an `/api/oauth/usage` payload: `five_hour` is the session window,
/// `seven_day` the weekly one; `utilization` is percent used (0-100).
fn parse_claude_usage(body: &Value) -> UsageSnapshot {
    let window = |key: &str| -> (Option<u8>, Option<u64>) {
        let Some(window) = body.get(key).filter(|window| window.is_object()) else {
            return (None, None);
        };
        let remaining = window
            .get("utilization")
            .and_then(Value::as_f64)
            .map(remaining_from_used);
        let reset_at = window
            .get("resets_at")
            .and_then(Value::as_str)
            .and_then(parse_rfc3339_epoch_secs);
        (remaining, reset_at)
    };
    let (five_hour_remaining, five_hour_reset_at) = window("five_hour");
    let (weekly_remaining, weekly_reset_at) = window("seven_day");
    UsageSnapshot {
        five_hour_remaining,
        weekly_remaining,
        five_hour_reset_at,
        weekly_reset_at,
    }
}

/// Parses an RFC 3339 / ISO 8601 timestamp such as
/// `2026-11-19T21:00:00.123456+00:00` or `...Z` into epoch seconds.
fn parse_rfc3339_epoch_secs(value: &str) -> Option<u64> {
    let value = value.trim();
    let bytes = value.as_bytes();
    if bytes.len() < 19 || bytes[4] != b'-' || bytes[7] != b'-' || bytes[13] != b':' || bytes[16] != b':' {
        return None;
    }
    if !matches!(bytes[10], b'T' | b't' | b' ') {
        return None;
    }
    let number = |range: std::ops::Range<usize>| -> Option<i64> {
        let text = value.get(range)?;
        text.bytes().all(|b| b.is_ascii_digit()).then(|| text.parse().ok())?
    };
    let (year, month, day) = (number(0..4)?, number(5..7)?, number(8..10)?);
    let (hour, minute, second) = (number(11..13)?, number(14..16)?, number(17..19)?);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    // Skip fractional seconds, then read the zone designator.
    let mut rest = &value[19..];
    if let Some(fraction) = rest.strip_prefix('.') {
        let digits = fraction.bytes().take_while(u8::is_ascii_digit).count();
        rest = &fraction[digits..];
    }
    let offset_secs = match rest {
        "Z" | "z" | "" => 0,
        zone => {
            let sign = match zone.as_bytes().first()? {
                b'+' => 1,
                b'-' => -1,
                _ => return None,
            };
            let zone = zone[1..].replace(':', "");
            if zone.len() != 4 || !zone.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            let hours: i64 = zone[..2].parse().ok()?;
            let minutes: i64 = zone[2..].parse().ok()?;
            sign * (hours * 3600 + minutes * 60)
        }
    };
    // Days from civil (Howard Hinnant's algorithm).
    let shifted_year = if month <= 2 { year - 1 } else { year };
    let era = shifted_year.div_euclid(400);
    let year_of_era = shifted_year - era * 400;
    let month_index = (month + 9) % 12;
    let day_of_year = (153 * month_index + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146_097 + day_of_era - 719_468;
    let secs = days * 86_400 + hour * 3600 + minute * 60 + second - offset_secs;
    u64::try_from(secs).ok()
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
        assert_eq!(UsageAgent::parse("Claude"), Some(UsageAgent::Claude));
        assert_eq!(UsageAgent::parse("hermes"), None);
        assert_eq!(UsageAgent::Codex.as_str(), "codex");
        assert_eq!(UsageAgent::Claude.as_str(), "claude");
        assert_eq!(
            UsageAgent::from_agent(crate::routing::AgentId::Claude),
            Some(UsageAgent::Claude)
        );
        assert_eq!(UsageAgent::from_agent(crate::routing::AgentId::Hermes), None);
    }

    #[test]
    fn parses_claude_usage_payload() {
        // Shape of `/api/oauth/usage` as Claude Code models it: each window
        // has a nullable `utilization` (percent used) and ISO `resets_at`.
        let body = json!({
            "five_hour": {"utilization": 41.6, "resets_at": "2026-11-19T21:00:00.512345+00:00"},
            "seven_day": {"utilization": 12.0, "resets_at": "2026-11-24T08:00:00Z"},
            "seven_day_opus": null,
            "extra_usage": {"is_enabled": false}
        });
        let usage = parse_claude_usage(&body);
        assert_eq!(usage.five_hour_remaining, Some(58));
        assert_eq!(usage.five_hour_reset_at, Some(1_795_122_000));
        assert_eq!(usage.weekly_remaining, Some(88));
        assert_eq!(usage.weekly_reset_at, Some(1_795_507_200));
    }

    #[test]
    fn claude_usage_tolerates_missing_and_null_windows() {
        assert_eq!(parse_claude_usage(&json!({})), UsageSnapshot::default());
        let usage = parse_claude_usage(&json!({
            "five_hour": {"utilization": null, "resets_at": null},
            "seven_day": null
        }));
        assert_eq!(usage, UsageSnapshot::default());
    }

    #[test]
    fn parses_rfc3339_offsets() {
        assert_eq!(parse_rfc3339_epoch_secs("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(
            parse_rfc3339_epoch_secs("2026-11-19T23:00:00+02:00"),
            Some(1_795_122_000)
        );
        assert_eq!(
            parse_rfc3339_epoch_secs("2024-02-29T12:00:00-05:30"),
            Some(1_709_227_800)
        );
        assert_eq!(parse_rfc3339_epoch_secs("not a date"), None);
        assert_eq!(parse_rfc3339_epoch_secs("2026-13-01T00:00:00Z"), None);
    }

    #[test]
    fn builds_claude_refresh_grant_from_stored_login() {
        let credentials = json!({"claudeAiOauth": {
            "accessToken": "old", "refreshToken": " r1 ", "expiresAt": 1,
            "scopes": ["user:inference", "user:profile"]
        }});
        let (refresh, body) = claude_refresh_request(&credentials).unwrap();
        assert_eq!(refresh, "r1");
        assert_eq!(body["grant_type"], "refresh_token");
        assert_eq!(body["refresh_token"], "r1");
        assert_eq!(body["client_id"], CLAUDE_OAUTH_CLIENT_ID);
        assert_eq!(body["scope"], "user:inference user:profile");
        let no_scopes = json!({"claudeAiOauth": {"refreshToken": "r1"}});
        assert!(claude_refresh_request(&no_scopes).unwrap().1.get("scope").is_none());
        assert!(claude_refresh_request(&json!({"claudeAiOauth": {"refreshToken": ""}})).is_none());
        assert!(claude_refresh_request(&json!({})).is_none());
    }

    #[test]
    fn claude_refresh_response_merges_into_credentials() {
        let credentials = json!({
            "claudeAiOauth": {
                "accessToken": "old", "refreshToken": "r1", "expiresAt": 1,
                "scopes": ["user:inference"], "subscriptionType": "max"
            },
            "mcpOAuth": {"kept": true}
        });
        let response = json!({
            "access_token": "new", "refresh_token": "r2", "expires_in": 28_800,
            "scope": "user:inference user:profile"
        });
        let updated = apply_claude_refresh(&credentials, &response, 1_000).unwrap();
        let oauth = &updated["claudeAiOauth"];
        assert_eq!(oauth["accessToken"], "new");
        assert_eq!(oauth["refreshToken"], "r2");
        assert_eq!(oauth["expiresAt"], 1_000 + 28_800_000);
        assert_eq!(oauth["scopes"], json!(["user:inference", "user:profile"]));
        assert_eq!(oauth["subscriptionType"], "max");
        assert_eq!(updated["mcpOAuth"]["kept"], true);
        // No rotated refresh token or scope: the stored ones stay.
        let updated =
            apply_claude_refresh(&credentials, &json!({"access_token": "new", "expires_in": 60}), 0)
                .unwrap();
        assert_eq!(updated["claudeAiOauth"]["refreshToken"], "r1");
        assert_eq!(updated["claudeAiOauth"]["scopes"], json!(["user:inference"]));
        // Malformed responses never produce credentials.
        assert!(apply_claude_refresh(&credentials, &json!({"expires_in": 60}), 0).is_none());
        assert!(apply_claude_refresh(&credentials, &json!({"access_token": "new"}), 0).is_none());
    }

    #[test]
    fn credentials_are_replaced_atomically() {
        let dir = std::env::temp_dir().join(format!("micro-emu-cred-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(".credentials.json");
        std::fs::write(&path, r#"{"claudeAiOauth":{"accessToken":"old"}}"#).unwrap();
        write_credentials_atomically(&path, &json!({"claudeAiOauth": {"accessToken": "new"}}))
            .unwrap();
        assert_eq!(read_json_file(&path).unwrap()["claudeAiOauth"]["accessToken"], "new");
        assert!(!path.with_extension("json.micro-emu-tmp").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn expired_claude_tokens_are_never_sent() {
        let credentials = json!({"claudeAiOauth": {"accessToken": "tok", "expiresAt": 2_000_000}});
        assert_eq!(
            claude_access_token_from(&credentials, 1_000_000).as_deref(),
            Some("tok")
        );
        assert_eq!(claude_access_token_from(&credentials, 1_990_000), None);
        assert_eq!(claude_access_token_from(&credentials, 3_000_000), None);
        assert_eq!(claude_access_token_from(&json!({}), 0), None);
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

//! A small blocking GitHub client: finding a token that works, and GraphQL.
//!
//! Tokens are tried in order: `JOUET_GITHUB_TOKEN`, `GH_TOKEN`,
//! `GITHUB_TOKEN`, then `gh auth token`. There is no separate probe: the
//! first real request checks the token, and a 401 moves on to the next
//! source, so a stale `GH_TOKEN` costs one rejected request rather than a
//! failed command. `JOUET_GITHUB_API_URL` points the client at another API
//! root, such as a GitHub Enterprise server or a test's fake server.

use serde::Deserialize;
use serde_json::Value;
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const ENV_SOURCES: [&str; 3] = ["JOUET_GITHUB_TOKEN", "GH_TOKEN", "GITHUB_TOKEN"];
const GH_SOURCE: &str = "gh auth token";
/// Retries after a 5xx, a dropped connection or a secondary rate limit.
const RETRIES: u32 = 3;
/// The longest a `Retry-After` header can make one retry wait.
const MAX_RETRY_AFTER: u64 = 60;

pub struct Client {
    agent: ureq::Agent,
    graphql_url: String,
    /// Token sources not yet tried, in order.
    sources: Vec<&'static str>,
    tried: Vec<String>,
    token: Option<Token>,
    rejected: Vec<&'static str>,
}

struct Token {
    source: &'static str,
    value: String,
}

#[derive(Deserialize)]
struct GraphqlResponse {
    data: Option<Value>,
    #[serde(default)]
    errors: Vec<GraphqlError>,
}

#[derive(Deserialize)]
struct GraphqlError {
    #[serde(rename = "type")]
    kind: Option<String>,
    message: String,
}

impl Client {
    pub fn from_env() -> Self {
        let api = std::env::var("JOUET_GITHUB_API_URL")
            .ok()
            .filter(|u| !u.is_empty())
            .unwrap_or_else(|| "https://api.github.com".into());
        let agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(30)))
            .user_agent(concat!("jouet/", env!("CARGO_PKG_VERSION")))
            .build()
            .into();
        Self {
            agent,
            graphql_url: format!("{}/graphql", api.trim_end_matches('/')),
            sources: ENV_SOURCES.into_iter().chain([GH_SOURCE]).collect(),
            tried: Vec::new(),
            token: None,
            rejected: Vec::new(),
        }
    }

    /// Runs a GraphQL query and returns its `data`. Paths GitHub reports as
    /// `NOT_FOUND` come back as `null` in the data; every other error fails
    /// the call.
    pub fn graphql(&mut self, query: &str, variables: &Value) -> Result<Value, String> {
        let body = serde_json::json!({ "query": query, "variables": variables }).to_string();
        loop {
            let token = match self.token.take() {
                Some(token) => token,
                None => self.next_token()?,
            };
            let (status, text) = self.post(&token.value, &body)?;
            if status == 401 {
                self.rejected.push(token.source);
                continue;
            }
            if !self.rejected.is_empty() {
                let rejected = std::mem::take(&mut self.rejected);
                let verb = if rejected.len() == 1 { "was" } else { "were" };
                eprintln!(
                    "jouet: {} {verb} rejected (401); using {}",
                    join_and(&rejected),
                    token.source
                );
            }
            self.token = Some(token);
            if status != 200 {
                return Err(format!("GitHub returned {status}: {}", message(&text)));
            }
            let resp: GraphqlResponse = serde_json::from_str(&text)
                .map_err(|e| format!("GitHub sent a response jouet could not read: {e}"))?;
            if let Some(e) = resp
                .errors
                .iter()
                .find(|e| e.kind.as_deref() != Some("NOT_FOUND"))
            {
                return Err(match e.kind.as_deref() {
                    Some("RATE_LIMITED") => format!("GitHub rate limit: {}", e.message),
                    _ => format!("GitHub: {}", e.message),
                });
            }
            return resp
                .data
                .ok_or_else(|| "GitHub returned no data".to_string());
        }
    }

    fn next_token(&mut self) -> Result<Token, String> {
        while !self.sources.is_empty() {
            let source = self.sources.remove(0);
            let value = if source == GH_SOURCE {
                gh_auth_token()
            } else {
                std::env::var(source).ok()
            };
            let Some(value) = value
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
            else {
                continue;
            };
            // The same token under two names would only be rejected twice.
            if self.tried.contains(&value) {
                continue;
            }
            self.tried.push(value.clone());
            return Ok(Token { source, value });
        }
        Err(if self.rejected.is_empty() {
            "no GitHub token: set JOUET_GITHUB_TOKEN, GH_TOKEN or GITHUB_TOKEN, or run `gh auth login`"
                .into()
        } else {
            format!(
                "GitHub rejected every token (401): {}; run `gh auth login` or set JOUET_GITHUB_TOKEN",
                join_and(&self.rejected)
            )
        })
    }

    /// POSTs `body`, retrying 5xx responses, dropped connections and
    /// secondary rate limits with jittered backoff.
    fn post(&self, token: &str, body: &str) -> Result<(u16, String), String> {
        let mut attempt = 0;
        loop {
            let sent = self
                .agent
                .post(&self.graphql_url)
                .header("Authorization", format!("bearer {token}"))
                .content_type("application/json")
                .send(body);
            let mut resp = match sent {
                Ok(resp) => resp,
                Err(e) if attempt < RETRIES => {
                    eprintln!("jouet: request to GitHub failed ({e}); retrying");
                    attempt += 1;
                    std::thread::sleep(backoff(attempt));
                    continue;
                }
                Err(e) => return Err(format!("could not reach {}: {e}", self.graphql_url)),
            };
            let status = resp.status().as_u16();
            let header = |name: &str| {
                resp.headers()
                    .get(name)
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_string)
            };
            let retry_after = header("retry-after").and_then(|v| v.parse::<u64>().ok());
            let remaining = header("x-ratelimit-remaining");
            let reset = header("x-ratelimit-reset").and_then(|v| v.parse::<i64>().ok());
            let text = resp
                .body_mut()
                .with_config()
                .limit(256 * 1024 * 1024)
                .read_to_string()
                .map_err(|e| format!("reading GitHub's response: {e}"))?;
            if status == 403 || status == 429 {
                if remaining.as_deref() == Some("0") {
                    let when = reset.map_or(String::new(), |r| {
                        format!(
                            "; it resets in {}",
                            crate::time::age(r - crate::time::now())
                        )
                    });
                    return Err(format!("GitHub rate limit exhausted{when}"));
                }
                let secondary = retry_after.is_some()
                    || text.to_ascii_lowercase().contains("secondary rate limit");
                if secondary && attempt < RETRIES {
                    attempt += 1;
                    let wait = retry_after.map_or_else(
                        || backoff(attempt),
                        |s| Duration::from_secs(s.min(MAX_RETRY_AFTER)),
                    );
                    eprintln!(
                        "jouet: GitHub secondary rate limit; retrying in {}s",
                        wait.as_secs()
                    );
                    std::thread::sleep(wait);
                    continue;
                }
                if secondary {
                    return Err(format!("GitHub secondary rate limit: {}", message(&text)));
                }
            }
            if status >= 500 && attempt < RETRIES {
                eprintln!("jouet: GitHub returned {status}; retrying");
                attempt += 1;
                std::thread::sleep(backoff(attempt));
                continue;
            }
            return Ok((status, text));
        }
    }
}

/// Reads a token from `gh`, with `GH_TOKEN` and `GITHUB_TOKEN` removed so
/// that gh reports its own stored token rather than echoing one that was
/// already tried.
fn gh_auth_token() -> Option<String> {
    let out = Command::new("gh")
        .args(["auth", "token"])
        .env_remove("GH_TOKEN")
        .env_remove("GITHUB_TOKEN")
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// `500ms · 2^(attempt-1)` plus up to 250ms of jitter.
fn backoff(attempt: u32) -> Duration {
    let jitter = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.subsec_millis() % 250);
    Duration::from_millis(500 * 2u64.pow(attempt - 1) + u64::from(jitter))
}

/// The `message` of a GitHub error body, or the body itself.
fn message(text: &str) -> String {
    serde_json::from_str::<Value>(text)
        .ok()
        .and_then(|v| v.get("message")?.as_str().map(str::to_string))
        .unwrap_or_else(|| text.trim().chars().take(200).collect())
}

fn join_and(items: &[&str]) -> String {
    match items {
        [] => String::new(),
        [one] => (*one).to_string(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_read_as_prose() {
        assert_eq!(join_and(&["GH_TOKEN"]), "GH_TOKEN");
        assert_eq!(
            join_and(&["GH_TOKEN", "GITHUB_TOKEN", "gh auth token"]),
            "GH_TOKEN, GITHUB_TOKEN and gh auth token"
        );
    }

    #[test]
    fn error_messages_come_from_the_body() {
        assert_eq!(
            message(r#"{"message":"Bad credentials"}"#),
            "Bad credentials"
        );
        assert_eq!(message("upstream timeout\n"), "upstream timeout");
    }
}

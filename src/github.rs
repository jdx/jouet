//! A small blocking GitHub client: finding a token that works, GraphQL, and
//! the few REST reads GraphQL has no equivalent for.
//!
//! Tokens are tried in order: `JOUET_GITHUB_TOKEN`, `GH_TOKEN`,
//! `GITHUB_TOKEN`, then `gh auth token`. There is no separate probe: the
//! first real request checks the token, and a 401 moves on to the next
//! source, so a stale `GH_TOKEN` costs one rejected request rather than a
//! failed command. `JOUET_GITHUB_API_URL` points the client at another API
//! root, such as a GitHub Enterprise server or a test's fake server.
//!
//! [`Client::get`] sends `If-None-Match` when given an ETag, so an unchanged
//! poll comes back as a 304 that costs nothing against the rate limit.

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
    api_url: String,
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

/// A REST response from [`Client::get`]. `status` is 304, with an empty
/// body, when the ETag given still matches.
#[derive(Debug)]
pub struct Rest {
    pub status: u16,
    pub etag: Option<String>,
    pub body: String,
}

/// Why a request got no usable response.
#[derive(Debug)]
pub enum Failure {
    /// The primary rate limit is used up until `reset`, in Unix seconds.
    RateLimited {
        reset: i64,
    },
    Other(String),
}

impl Failure {
    pub fn message(&self) -> String {
        match self {
            Self::RateLimited { reset } => format!(
                "GitHub rate limit exhausted; it resets in {}",
                crate::time::age(reset - crate::time::now())
            ),
            Self::Other(message) => message.clone(),
        }
    }
}

impl From<Failure> for String {
    fn from(f: Failure) -> Self {
        f.message()
    }
}

/// A response: its status, ETag and body.
struct Sent {
    status: u16,
    etag: Option<String>,
    text: String,
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
            .tls_config(crate::tls::config())
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(30)))
            .user_agent(concat!("jouet/", env!("CARGO_PKG_VERSION")))
            .build()
            .into();
        Self {
            agent,
            api_url: api.trim_end_matches('/').to_string(),
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
        self.graphql_with(query, variables, true)
    }

    /// Runs a GraphQL mutation. Unlike a query it is not retried after a 5xx
    /// or a dropped connection, which can follow a write that succeeded.
    pub fn mutate(&mut self, query: &str, variables: &Value) -> Result<Value, String> {
        self.graphql_with(query, variables, false)
    }

    fn graphql_with(
        &mut self,
        query: &str,
        variables: &Value,
        idempotent: bool,
    ) -> Result<Value, String> {
        let body = serde_json::json!({ "query": query, "variables": variables }).to_string();
        let url = format!("{}/graphql", self.api_url);
        let Sent { status, text, .. } = self.send(&url, Some(&body), None, idempotent)?;
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
        resp.data
            .ok_or_else(|| "GitHub returned no data".to_string())
    }

    /// GETs a REST path such as `/repos/OWNER/NAME/activity`. A 404 comes
    /// back as `None`.
    pub fn rest_get(&mut self, path: &str) -> Result<Option<Value>, String> {
        let url = format!("{}{path}", self.api_url);
        let Sent { status, text, .. } = self.send(&url, None, None, true)?;
        match status {
            200 => serde_json::from_str(&text)
                .map(Some)
                .map_err(|e| format!("GitHub sent a response jouet could not read: {e}")),
            404 => Ok(None),
            _ => Err(format!("GitHub returned {status}: {}", message(&text))),
        }
    }

    /// GETs a REST path such as `/repos/o/r/pulls/1`, or a full URL,
    /// sending `If-None-Match` when `etag` is given. Statuses other than 401
    /// are the caller's to read, and an exhausted rate limit says when it
    /// resets, so a caller can wait it out.
    pub fn get(&mut self, path: &str, etag: Option<&str>) -> Result<Rest, Failure> {
        let url = if path.starts_with('/') {
            format!("{}{path}", self.api_url)
        } else {
            path.to_string()
        };
        let sent = self.send(&url, None, etag, true)?;
        Ok(Rest {
            status: sent.status,
            etag: sent.etag,
            body: sent.text,
        })
    }

    /// Sends a request with the first token GitHub accepts: a POST of `body`
    /// when there is one, else a GET (conditional when `etag` is given).
    fn send(
        &mut self,
        url: &str,
        body: Option<&str>,
        etag: Option<&str>,
        idempotent: bool,
    ) -> Result<Sent, Failure> {
        loop {
            let token = match self.token.take() {
                Some(token) => token,
                None => self.next_token().map_err(Failure::Other)?,
            };
            let sent = match self.request(&token.value, url, body, etag, idempotent) {
                Ok(sent) => sent,
                // The token was not rejected, so it is kept for the next
                // request (such as the one after a rate limit resets).
                Err(failure) => {
                    self.token = Some(token);
                    return Err(failure);
                }
            };
            if sent.status == 401 {
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
            return Ok(sent);
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

    /// Sends one request, retrying secondary rate limits with jittered
    /// backoff, and when it is `idempotent` 5xx responses and connections
    /// dropped before or during the response too.
    fn request(
        &self,
        token: &str,
        url: &str,
        body: Option<&str>,
        etag: Option<&str>,
        idempotent: bool,
    ) -> Result<Sent, Failure> {
        let mut attempt = 0;
        loop {
            let auth = format!("bearer {token}");
            let sent = match body {
                Some(body) => self
                    .agent
                    .post(url)
                    .header("Authorization", auth)
                    .content_type("application/json")
                    .send(body),
                None => {
                    let mut req = self
                        .agent
                        .get(url)
                        .header("Authorization", auth)
                        .header("Accept", "application/vnd.github+json")
                        .header("X-GitHub-Api-Version", "2022-11-28");
                    if let Some(etag) = etag {
                        req = req.header("If-None-Match", etag);
                    }
                    req.call()
                }
            }
            .and_then(|mut resp| {
                let text = resp
                    .body_mut()
                    .with_config()
                    .limit(256 * 1024 * 1024)
                    .read_to_string()?;
                Ok((resp, text))
            });
            let (resp, text) = match sent {
                Ok(sent) => sent,
                Err(e) if idempotent && attempt < RETRIES => {
                    eprintln!("jouet: request to GitHub failed ({e}); retrying");
                    attempt += 1;
                    std::thread::sleep(backoff(attempt));
                    continue;
                }
                Err(e) => return Err(Failure::Other(format!("could not reach {url}: {e}"))),
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
            let etag = header("etag");
            if status == 403 || status == 429 {
                if remaining.as_deref() == Some("0") {
                    let reset = reset.unwrap_or_else(|| crate::time::now() + 60);
                    return Err(Failure::RateLimited { reset });
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
                    return Err(Failure::Other(format!(
                        "GitHub secondary rate limit: {}",
                        message(&text)
                    )));
                }
            }
            if status >= 500 && idempotent && attempt < RETRIES {
                eprintln!("jouet: GitHub returned {status}; retrying");
                attempt += 1;
                std::thread::sleep(backoff(attempt));
                continue;
            }
            return Ok(Sent { status, etag, text });
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

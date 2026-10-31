//! A blocking REST client for GitLab and Forgejo, shaped like [`Client`]:
//! tokens tried in order, a 401 moving on to the next source, retries after
//! a 5xx or a dropped connection, and pages followed through the `Link`
//! header.
//!
//! A host's tokens come from the environment variables that belong to it
//! ([`forge::token_vars`](crate::forge): GitLab's to `GITLAB_HOST` or
//! gitlab.com, `FORGEJO_TOKEN` to codeberg.org, `GITEA_TOKEN` to gitea.com,
//! a configured host's `token_env` to that host), then from glab's or tea's
//! `config.yml`, which keep a token per host. No token is ever sent to a
//! host it does not belong to. With no token at all a request goes without
//! one, which public projects answer. Messages name a token's source, never
//! its value.
//!
//! [`Client`]: crate::Client

use crate::forge::{self, Forge};
use crate::{Failure, Method, backoff, join_and, message};
use serde_json::Value;
use std::path::PathBuf;
use std::time::Duration;

/// Retries after a 5xx, a dropped connection or a 429.
const RETRIES: u32 = 3;
/// The longest a `Retry-After` header can make one retry wait.
const MAX_RETRY_AFTER: u64 = 60;
/// Pages of a list read at most.
pub const MAX_PAGES: usize = 30;

pub struct Rest {
    agent: ureq::Agent,
    forge: Forge,
    host: String,
    api_url: String,
    /// Token sources not yet tried, in order.
    sources: Vec<Source>,
    tried: Vec<String>,
    token: Option<Token>,
    rejected: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Source {
    Env(String),
    /// glab's or tea's `config.yml`.
    Cli,
    /// Every source used up: requests go without a token.
    Anonymous,
}

struct Token {
    source: Source,
    /// `None` sends the request without a token.
    value: Option<String>,
}

/// A REST response. `status` is 304, with an empty body, when the ETag
/// given still matches.
#[derive(Debug)]
pub struct Response {
    pub status: u16,
    pub etag: Option<String>,
    pub link: Option<String>,
    pub body: String,
}

impl Rest {
    pub fn new(forge: Forge, host: &str) -> Self {
        let agent = ureq::Agent::config_builder()
            .tls_config(crate::tls::config())
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(30)))
            .user_agent(concat!("jouet/", env!("CARGO_PKG_VERSION")))
            .build()
            .into();
        let sources = forge::token_vars(forge, host)
            .into_iter()
            .map(Source::Env)
            .chain([Source::Cli])
            .collect();
        Self {
            agent,
            forge,
            host: forge::normalize(host),
            api_url: forge::api_url(forge, host),
            sources,
            tried: Vec::new(),
            token: None,
            rejected: Vec::new(),
        }
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    /// The web server's root: the API root without its `/api/v4` or
    /// `/api/v1`, else `https://HOST`.
    pub fn web_root(&self) -> String {
        ["/api/v4", "/api/v1"]
            .iter()
            .find_map(|suffix| self.api_url.strip_suffix(suffix))
            .map_or_else(|| format!("https://{}", self.host), str::to_string)
    }

    /// GETs a path under the API root, or a full URL (a next page),
    /// sending `If-None-Match` when `etag` is given. Statuses other than 401
    /// are the caller's to read.
    pub fn get(&mut self, path: &str, etag: Option<&str>) -> Result<Response, Failure> {
        let url = self.url(path);
        self.send(&url, None, etag, true)
    }

    /// GETs JSON; a 404 is `None`, and any other status but 200 an error.
    pub fn json(&mut self, path: &str) -> Result<Option<Value>, Failure> {
        let resp = self.get(path, None)?;
        self.read(path, resp)
    }

    /// Every page of a JSON list, up to [`MAX_PAGES`]; a 404 is `None`.
    pub fn list(&mut self, path: &str) -> Result<Option<Vec<Value>>, Failure> {
        let mut out = Vec::new();
        let mut next = Some(path.to_string());
        let mut pages = 0;
        while let Some(path) = next.take() {
            let resp = self.get(&path, None)?;
            let link = resp.link.clone();
            let Some(page) = self.read(&path, resp)? else {
                return Ok(None);
            };
            out.extend(page.as_array().cloned().unwrap_or_default());
            pages += 1;
            if pages < MAX_PAGES {
                next = link.as_deref().and_then(crate::next_page);
            }
        }
        Ok(Some(out))
    }

    /// Sends a JSON body with a method that writes. It is not retried after
    /// a 5xx or a dropped connection, which can follow a write that
    /// succeeded.
    pub fn write(&mut self, method: Method, path: &str, body: &Value) -> Result<Response, Failure> {
        let url = self.url(path);
        let body = body.to_string();
        self.send(&url, Some((method, &body)), None, false)
    }

    /// Reads a response as JSON: 404 is `None`, other statuses but 2xx are
    /// errors carrying the forge's message.
    pub fn read(&self, path: &str, resp: Response) -> Result<Option<Value>, Failure> {
        match resp.status {
            200..=299 => serde_json::from_str(&resp.body).map(Some).map_err(|e| {
                Failure::Other(format!(
                    "{} sent {path} in a form jouet could not read: {e}",
                    self.forge.name()
                ))
            }),
            404 => Ok(None),
            status => Err(self.refused(status, &resp.body)),
        }
    }

    /// A failure for an unexpected status, with the forge's message.
    pub fn refused(&self, status: u16, body: &str) -> Failure {
        Failure::Other(format!(
            "{} returned {status}: {}",
            self.forge.name(),
            error_message(body)
        ))
    }

    fn url(&self, path: &str) -> String {
        if path.starts_with('/') {
            format!("{}{path}", self.api_url)
        } else {
            path.to_string()
        }
    }

    fn send(
        &mut self,
        url: &str,
        body: Option<(Method, &str)>,
        etag: Option<&str>,
        idempotent: bool,
    ) -> Result<Response, Failure> {
        loop {
            let token = match self.token.take() {
                Some(token) => token,
                None => self.next_token(),
            };
            let sent = match self.request(token.value.as_deref(), url, body, etag, idempotent) {
                Ok(sent) => sent,
                Err(failure) => {
                    self.token = Some(token);
                    return Err(failure);
                }
            };
            if sent.status == 401 {
                if token.value.is_none() {
                    return Err(Failure::Other(self.no_token()));
                }
                self.rejected.push(self.source_name(&token.source));
                continue;
            }
            if !self.rejected.is_empty() && token.value.is_some() {
                let rejected = std::mem::take(&mut self.rejected);
                let names: Vec<&str> = rejected.iter().map(String::as_str).collect();
                let verb = if names.len() == 1 { "was" } else { "were" };
                eprintln!(
                    "jouet: {} {verb} rejected by {} (401); using {}",
                    join_and(&names),
                    self.host,
                    self.source_name(&token.source)
                );
            }
            self.token = Some(token);
            return Ok(sent);
        }
    }

    /// The next token to try; once every source is used up, none, so a
    /// public project can still be read.
    fn next_token(&mut self) -> Token {
        while !self.sources.is_empty() {
            let source = self.sources.remove(0);
            let value = match &source {
                Source::Env(var) => std::env::var(var).ok(),
                Source::Cli => cli_token(self.forge, &self.host),
                Source::Anonymous => None,
            };
            let Some(value) = value
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
            else {
                continue;
            };
            if self.tried.contains(&value) {
                continue;
            }
            self.tried.push(value.clone());
            return Token {
                source,
                value: Some(value),
            };
        }
        Token {
            source: Source::Anonymous,
            value: None,
        }
    }

    fn source_name(&self, source: &Source) -> String {
        match (source, self.forge) {
            (Source::Env(var), _) => var.to_string(),
            (Source::Cli, Forge::GitLab) => "glab's config".into(),
            (Source::Cli, _) => "tea's config".into(),
            (Source::Anonymous, _) => "no token".into(),
        }
    }

    /// Why a request that needed a token got none, naming what was tried.
    fn no_token(&self) -> String {
        let vars = forge::token_vars(self.forge, &self.host);
        let vars = if vars.is_empty() {
            format!(
                "the variable named by token_env in [forges.\"{}\"]",
                self.host
            )
        } else {
            join_or(&vars.iter().map(String::as_str).collect::<Vec<_>>())
        };
        let cli = match self.forge {
            Forge::GitLab => "run `glab auth login`",
            _ => "run `tea login add`",
        };
        if self.rejected.is_empty() {
            format!("{} needs a token: set {vars}, or {cli}", self.host)
        } else {
            let names: Vec<&str> = self.rejected.iter().map(String::as_str).collect();
            format!(
                "{} rejected every token (401): {}; set {vars}, or {cli}",
                self.host,
                join_and(&names)
            )
        }
    }

    fn request(
        &self,
        token: Option<&str>,
        url: &str,
        body: Option<(Method, &str)>,
        etag: Option<&str>,
        idempotent: bool,
    ) -> Result<Response, Failure> {
        let auth = token.map(|t| match self.forge {
            Forge::Forgejo => format!("token {t}"),
            _ => format!("Bearer {t}"),
        });
        let mut attempt = 0;
        loop {
            let sent = match body {
                Some((method, body)) => {
                    let req = match method {
                        Method::Post => self.agent.post(url),
                        Method::Put => self.agent.put(url),
                        Method::Patch => self.agent.patch(url),
                        Method::Delete => self.agent.delete(url).force_send_body(),
                    };
                    let req = match &auth {
                        Some(auth) => req.header("Authorization", auth),
                        None => req,
                    };
                    req.header("Accept", "application/json")
                        .content_type("application/json")
                        .send(body)
                }
                None => {
                    let mut req = self.agent.get(url).header("Accept", "application/json");
                    if let Some(auth) = &auth {
                        req = req.header("Authorization", auth);
                    }
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
            let name = self.forge.name();
            let (resp, text) = match sent {
                Ok(sent) => sent,
                Err(e) if idempotent && attempt < RETRIES => {
                    eprintln!("jouet: request to {name} failed ({e}); retrying");
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
            if status == 429 {
                let retry_after = header("retry-after").and_then(|v| v.parse::<u64>().ok());
                if attempt < RETRIES {
                    attempt += 1;
                    let wait = retry_after.map_or_else(
                        || backoff(attempt),
                        |s| Duration::from_secs(s.min(MAX_RETRY_AFTER)),
                    );
                    eprintln!("jouet: {name} rate limit; retrying in {}s", wait.as_secs());
                    std::thread::sleep(wait);
                    continue;
                }
                return Err(Failure::Other(format!(
                    "{name} rate limit: {}",
                    error_message(&text)
                )));
            }
            if status >= 500 && idempotent && attempt < RETRIES {
                eprintln!("jouet: {name} returned {status}; retrying");
                attempt += 1;
                std::thread::sleep(backoff(attempt));
                continue;
            }
            return Ok(Response {
                status,
                etag: header("etag"),
                link: header("link"),
                body: text,
            });
        }
    }
}

/// The `message` (GitLab, Forgejo) or `error` of an error body, or the
/// start of the body.
fn error_message(body: &str) -> String {
    let parsed = serde_json::from_str::<Value>(body).ok();
    let field = parsed.as_ref().and_then(|v| {
        ["message", "error"].iter().find_map(|k| match &v[*k] {
            Value::String(s) => Some(s.clone()),
            Value::Null => None,
            other => Some(other.to_string()),
        })
    });
    field.unwrap_or_else(|| message(body))
}

/// A timestamp sent with the server's offset (`+01:00`), in UTC as GitHub
/// sends them.
pub fn utc(v: &Value) -> Option<String> {
    v.as_str()
        .and_then(jouet_output::time::parse_rfc3339)
        .map(jouet_output::time::format_utc)
}

/// Percent-encodes everything but unreserved characters, for a path segment
/// such as GitLab's `group%2Fproject` or a query value.
pub fn encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn join_or(items: &[&str]) -> String {
    match items {
        [] => String::new(),
        [one] => (*one).to_string(),
        [rest @ .., last] => format!("{} or {last}", rest.join(", ")),
    }
}

/// Reads a CLI's config for the token of a host.
type ReadToken = fn(&str, &str) -> Option<String>;

/// The token glab (for GitLab) or tea (for Forgejo) keeps for `host`.
fn cli_token(forge: Forge, host: &str) -> Option<String> {
    let (dir, read): (&str, ReadToken) = match forge {
        Forge::GitLab => ("glab-cli", glab_token),
        Forge::Forgejo => ("tea", tea_token),
        Forge::GitHub | Forge::Entire => return None,
    };
    let override_dir = match forge {
        Forge::GitLab => std::env::var_os("GLAB_CONFIG_DIR").map(PathBuf::from),
        _ => None,
    };
    let mut dirs: Vec<PathBuf> = override_dir.into_iter().collect();
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from) {
        dirs.push(xdg.join(dir));
    }
    if let Some(home) = std::env::home_dir() {
        dirs.push(home.join(".config").join(dir));
        dirs.push(home.join("Library/Application Support").join(dir));
    }
    dirs.iter()
        .filter_map(|d| std::fs::read_to_string(d.join("config.yml")).ok())
        .find_map(|text| read(&text, host))
}

/// A YAML scalar without its quotes.
fn unquote(value: &str) -> String {
    let v = value.trim();
    let v = v
        .strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .or_else(|| v.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))
        .unwrap_or(v);
    v.to_string()
}

/// A `key: value` line, or `key:` with an empty value.
fn key_value(line: &str) -> Option<(String, String)> {
    let line = line.trim();
    if let Some(key) = line.strip_suffix(':') {
        return Some((unquote(key), String::new()));
    }
    let (key, value) = line.split_once(": ")?;
    Some((unquote(key), unquote(value)))
}

/// `hosts.<host>.token` in glab's config.
fn glab_token(text: &str, host: &str) -> Option<String> {
    let mut in_hosts = false;
    let mut host_indent: Option<usize> = None;
    let mut current: Option<String> = None;
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let indent = line.len() - trimmed.len();
        if indent == 0 {
            in_hosts = trimmed.trim_end() == "hosts:";
            host_indent = None;
            current = None;
            continue;
        }
        let Some((key, value)) = key_value(trimmed).filter(|_| in_hosts) else {
            continue;
        };
        if host_indent.is_none_or(|h| indent <= h) {
            host_indent = Some(indent);
            current = Some(forge::normalize(&key));
        } else if key == "token"
            && current.as_deref() == Some(host)
            && !value.is_empty()
            && !value.starts_with("!!null")
        {
            return Some(value);
        }
    }
    None
}

/// The token of the login in tea's config whose `url` is on `host`.
fn tea_token(text: &str, host: &str) -> Option<String> {
    let mut in_logins = false;
    let mut logins: Vec<(Option<String>, Option<String>)> = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if line.len() == trimmed.len() {
            in_logins = trimmed.trim_end() == "logins:";
            continue;
        }
        if !in_logins {
            continue;
        }
        let entry = match trimmed.strip_prefix("- ") {
            Some(rest) => {
                logins.push((None, None));
                rest
            }
            None => trimmed,
        };
        let (Some(login), Some((key, value))) = (logins.last_mut(), key_value(entry)) else {
            continue;
        };
        match key.as_str() {
            "url" => login.0 = Some(value),
            "token" => login.1 = Some(value),
            _ => {}
        }
    }
    logins.into_iter().find_map(|(url, token)| {
        let url = url?;
        let rest = url.split_once("://").map_or(url.as_str(), |(_, r)| r);
        let login_host = rest.split('/').next()?;
        (forge::normalize(login_host) == host)
            .then_some(token)
            .flatten()
            .filter(|t| !t.is_empty())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glab_keeps_a_token_per_host() {
        let config = "git_protocol: ssh\nhosts:\n    gitlab.com:\n        api_protocol: https\n        token: glpat-one\n    git.example.com:\n        token: 'glpat-two'\n";
        assert_eq!(
            glab_token(config, "gitlab.com").as_deref(),
            Some("glpat-one")
        );
        assert_eq!(
            glab_token(config, "git.example.com").as_deref(),
            Some("glpat-two")
        );
        assert_eq!(glab_token(config, "other.example.com"), None);
        // A token kept in the keyring leaves the field empty.
        assert_eq!(
            glab_token("hosts:\n  gitlab.com:\n    token:\n", "gitlab.com"),
            None
        );
    }

    #[test]
    fn tea_matches_a_login_by_its_url() {
        let config = "logins:\n    - name: work\n      url: https://git.example.com\n      token: one\n    - name: codeberg\n      url: https://codeberg.org\n      token: \"two\"\n      default: true\npreferences:\n    editor: false\n";
        assert_eq!(tea_token(config, "codeberg.org").as_deref(), Some("two"));
        assert_eq!(tea_token(config, "git.example.com").as_deref(), Some("one"));
        assert_eq!(tea_token(config, "gitea.com"), None);
    }

    #[test]
    fn error_bodies_name_the_forges_message() {
        assert_eq!(
            error_message(r#"{"message":"401 Unauthorized"}"#),
            "401 Unauthorized"
        );
        assert_eq!(
            error_message(r#"{"message":{"base":["is behind"]}}"#),
            r#"{"base":["is behind"]}"#
        );
        assert_eq!(
            error_message(r#"{"error":"insufficient_scope"}"#),
            "insufficient_scope"
        );
    }
}

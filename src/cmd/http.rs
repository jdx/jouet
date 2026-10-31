//! `jouet http METHOD URL`: one request, its status and only the parts of the
//! body asked for, within a budget, with hurl-style assertions. Credentials
//! are redacted from everything it prints or saves.

use super::Output;
use crate::HttpArgs;
use crate::redact::{self, MASK};
use jouet_output::display::Mode;
use jouet_output::exit::Exit;
use jouet_wait::http::{self, Assertion, Checked, Failure};
use regex::Regex;
use serde::Serialize;
use serde_json::Value;
use std::fmt::Write as _;
use std::io::Read as _;
use std::path::PathBuf;
use std::sync::LazyLock;
use std::time::Duration;

/// Bytes of body or selected values printed unless --budget says otherwise.
const DEFAULT_BUDGET: usize = 4000;
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
/// A larger body is refused rather than read into memory.
const BODY_LIMIT: u64 = 64 << 20;
const METHODS: &[&str] = &["GET", "HEAD", "POST", "PUT", "PATCH", "DELETE", "OPTIONS"];
/// Response headers shown without --headers, besides rate limits when they
/// run low: the ones an agent acts on.
const KEY_HEADERS: &[&str] = &["location", "retry-after", "www-authenticate"];
/// Headers whose whole value is a credential.
const SECRET_HEADERS: &[&str] = &[
    "authorization",
    "proxy-authorization",
    "cookie",
    "set-cookie",
];
/// Words in a header name or JSON key that say its value is a secret.
const SECRET_WORDS: &[&str] = &[
    "token",
    "secret",
    "password",
    "passwd",
    "api_key",
    "apikey",
    "api-key",
    "private_key",
    "private-key",
    "credential",
    "session",
];
/// Secret values shorter than this are not searched for, so a one-letter
/// variable does not mask every `a` in the body.
const MIN_SECRET: usize = 4;
/// Rate-limit headers are shown once fewer requests than this are left.
const RATE_LOW: u64 = 10;

static VAR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\$\{([A-Za-z_][A-Za-z0-9_]*)\}").expect("the pattern compiles"));

#[derive(Serialize)]
struct Report {
    method: String,
    /// As given, before `${NAME}` was expanded.
    url: String,
    status: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<&'static str>,
    elapsed_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    content_type: Option<String>,
    body_bytes: usize,
    /// Every response header, a repeated one's values joined with `, `.
    headers: serde_json::Map<String, Value>,
    /// The body as JSON, when it is JSON, nothing was selected and it fits
    /// the budget.
    #[serde(skip_serializing_if = "Option::is_none")]
    body: Option<Value>,
    /// The body as text otherwise, cut to the budget.
    #[serde(skip_serializing_if = "Option::is_none")]
    body_text: Option<String>,
    /// How much of `body_text` was cut.
    #[serde(skip_serializing_if = "is_zero")]
    omitted_bytes: usize,
    /// Where the whole body is saved, when anything was cut.
    #[serde(skip_serializing_if = "Option::is_none")]
    body_file: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    selected: Vec<Selected>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    assertions: Vec<Checked>,
    #[serde(skip)]
    shown_headers: Vec<(String, String)>,
    #[serde(skip)]
    binary: bool,
    /// `body` as it is printed: indented for people.
    #[serde(skip)]
    rendered: Option<String>,
}

#[derive(Serialize)]
struct Selected {
    path: String,
    values: Vec<Value>,
    /// Matches left out to stay within the budget.
    #[serde(skip_serializing_if = "is_zero")]
    omitted: usize,
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

pub(crate) fn run(args: &HttpArgs, out: &Output) -> Exit {
    match request(args, out) {
        Ok(exit) => exit,
        Err((exit, message)) => out.error(exit, &message),
    }
}

type Refusal = (Exit, String);

fn usage(message: String) -> Refusal {
    (Exit::Usage, message)
}

fn request(args: &HttpArgs, out: &Output) -> Result<Exit, Refusal> {
    let method = args.method.to_ascii_uppercase();
    if !METHODS.contains(&method.as_str()) {
        return Err(usage(format!(
            "`{}` is not an HTTP method: give one of {}",
            args.method,
            METHODS.join(", ")
        )));
    }
    let timeout = match &args.timeout {
        Some(t) => jouet_output::time::parse_duration(t).map_err(usage)?,
        None => DEFAULT_TIMEOUT,
    };
    let selects = args
        .select
        .iter()
        .map(|s| http::select(s).map(|p| (s.clone(), p)))
        .collect::<Result<Vec<_>, _>>()
        .map_err(usage)?;
    let assertions = args
        .expect
        .iter()
        .map(|e| Assertion::parse(e))
        .collect::<Result<Vec<_>, _>>()
        .map_err(usage)?;
    let mut secrets = Secrets::from_fnox();
    let url = full_url(&secrets.expand(&args.url)?).map_err(usage)?;
    let mut headers = Vec::new();
    for h in &args.header {
        let Some((name, value)) = h.split_once(':') else {
            return Err(usage(format!("`--header {h}` is not NAME:VALUE")));
        };
        let name = name.trim().to_ascii_lowercase();
        if name.is_empty() || !name.bytes().all(|b| b.is_ascii_graphic()) {
            return Err(usage(format!("`--header {h}` has no valid name")));
        }
        let value = secrets.expand(value.trim())?;
        if secret_name(&name) {
            secrets.add(&value);
            // `Bearer TOKEN`: a server may echo the token alone.
            if let Some((_, credential)) = value.split_once(' ') {
                secrets.add(credential.trim());
            }
        }
        headers.push((name, value));
    }
    let body = match (&args.json_body, &args.data) {
        (Some(_), Some(_)) => {
            return Err(usage("give --json-body or --data, not both".into()));
        }
        (Some(json), None) => {
            let bytes = body_arg(json, "--json-body", &mut secrets)?;
            if let Err(e) = serde_json::from_slice::<Value>(&bytes) {
                return Err(usage(format!("--json-body is not JSON: {e}")));
            }
            if !headers.iter().any(|(n, _)| n == "content-type") {
                headers.push(("content-type".into(), "application/json".into()));
            }
            Some(bytes)
        }
        (None, Some(data)) => Some(body_arg(data, "--data", &mut secrets)?),
        (None, None) => None,
    };
    let agent = http::agent(!args.no_follow);
    let resp = http::send(
        &agent,
        &http::Request {
            method: &method,
            url: &url,
            headers: &headers,
            body: body.as_deref(),
            timeout,
            body_limit: BODY_LIMIT,
        },
    )
    .map_err(|f| match f {
        Failure::Timeout(m) => (
            Exit::Timeout,
            format!("{method} {}: {m}", secrets.hide(&args.url)),
        ),
        Failure::Unreachable(m) => (
            Exit::Remote,
            format!("{method} {}: {}", secrets.hide(&args.url), secrets.hide(&m)),
        ),
        Failure::Invalid(m) => usage(secrets.hide(&m)),
    })?;

    let json: Option<Value> = serde_json::from_slice(&resp.body).ok();
    let facts = resp.facts(json.as_ref());
    let mut checks: Vec<Checked> = assertions.iter().map(|a| a.check(&facts)).collect();
    if !assertions.iter().any(Assertion::is_status) && resp.status >= 400 {
        checks.push(Checked {
            assertion: "status < 400".into(),
            passed: false,
            actual: Some(resp.status.into()),
            reason: Some(format!(
                "got {0}; give --expect status={0} to accept it",
                resp.status
            )),
        });
    }
    for check in &mut checks {
        if let Some(actual) = &mut check.actual {
            secrets.hide_value(actual);
        }
        check.reason = check.reason.as_deref().map(|r| secrets.hide(r));
    }
    let exit = if checks.iter().all(|c| c.passed) {
        Exit::Ok
    } else {
        Exit::Failed
    };

    let pretty = out.mode == Mode::Human && !out.json;
    let budget = match args.budget {
        Some(0) => usize::MAX,
        Some(n) => n,
        None => DEFAULT_BUDGET,
    };
    let mut report = Report {
        method,
        url: secrets.hide(&args.url),
        status: resp.status,
        reason: resp.reason(),
        elapsed_ms: u64::try_from(resp.elapsed.as_millis()).unwrap_or(u64::MAX),
        content_type: resp.header("content-type").map(str::to_string),
        body_bytes: resp.body.len(),
        headers: serde_json::Map::new(),
        body: None,
        body_text: None,
        omitted_bytes: 0,
        body_file: None,
        selected: Vec::new(),
        assertions: checks,
        shown_headers: Vec::new(),
        binary: false,
        rendered: None,
    };
    let rate_low = resp.status == 429
        || resp
            .headers
            .iter()
            .filter(|(n, _)| n.ends_with("ratelimit-remaining"))
            .any(|(_, v)| v.trim().parse::<u64>().is_ok_and(|left| left < RATE_LOW));
    for (name, value) in &resp.headers {
        let value = if secret_name(name) {
            MASK.to_string()
        } else {
            secrets.hide(value)
        };
        let rate = name.ends_with("ratelimit-remaining") || name.ends_with("ratelimit-reset");
        if args.headers || KEY_HEADERS.contains(&name.as_str()) || (rate && rate_low) {
            report.shown_headers.push((name.clone(), value.clone()));
        }
        match report.headers.get_mut(name) {
            Some(Value::String(joined)) => {
                joined.push_str(", ");
                joined.push_str(&value);
            }
            _ => {
                report.headers.insert(name.clone(), value.into());
            }
        }
    }

    // What is printed, and saved, is the redacted body.
    let shown = json.map(|mut v| {
        secrets.hide_value(&mut v);
        v
    });
    // A failed status shows the body too: an error page says why.
    if selects.is_empty() || resp.status >= 400 {
        body_report(
            &mut report,
            &resp.body,
            shown.as_ref(),
            &secrets,
            budget,
            pretty,
        );
    }
    if !selects.is_empty() {
        select_report(&mut report, &resp.body, shown.as_ref(), &selects, budget);
    }
    out.result(&report, || render(&report));
    Ok(exit)
}

/// The body as JSON when it fits, else as text cut to the budget.
fn body_report(
    report: &mut Report,
    raw: &[u8],
    json: Option<&Value>,
    secrets: &Secrets,
    budget: usize,
    pretty: bool,
) {
    if raw.is_empty() {
        return;
    }
    let text = match json {
        Some(v) if pretty => serde_json::to_string_pretty(v).unwrap_or_default(),
        Some(v) => v.to_string(),
        None => match std::str::from_utf8(raw) {
            Ok(text) => secrets.hide(text),
            Err(_) => {
                report.binary = true;
                report.body_file = save(raw, "bin");
                return;
            }
        },
    };
    if text.len() <= budget {
        match json {
            Some(v) => {
                report.body = Some(v.clone());
                report.rendered = Some(text);
            }
            None => report.body_text = Some(text),
        }
        return;
    }
    let end = floor_char_boundary(&text, budget);
    report.omitted_bytes = text.len() - end;
    report.body_file = save_text(&text, json);
    report.body_text = Some(text[..end].to_string());
}

/// The values at each JSONPath, as many as fit the budget.
fn select_report(
    report: &mut Report,
    raw: &[u8],
    json: Option<&Value>,
    selects: &[(String, http::JsonPath)],
    budget: usize,
) {
    let mut used = 0usize;
    for (text, path) in selects {
        let mut selected = Selected {
            path: text.clone(),
            values: Vec::new(),
            omitted: 0,
        };
        if let Some(json) = json {
            for value in path.query(json).all() {
                let len = value.to_string().len();
                // The first value is shown even past the budget: an answer
                // that is one large value is still the answer.
                if used > 0 && used.saturating_add(len) > budget {
                    selected.omitted += 1;
                    continue;
                }
                used = used.saturating_add(len);
                selected.values.push(value.clone());
            }
        }
        report.selected.push(selected);
    }
    if report.body_file.is_none()
        && report.selected.iter().any(|s| s.omitted > 0)
        && !raw.is_empty()
    {
        report.body_file = json.and_then(|v| save_text(&v.to_string(), Some(v)));
    }
}

fn render(r: &Report) -> String {
    let mut out = format!("HTTP {}", r.status);
    if let Some(reason) = r.reason {
        let _ = write!(out, " {reason}");
    }
    if let Some(ct) = &r.content_type {
        let _ = write!(out, ", {ct}");
    }
    let _ = writeln!(out, ", {} bytes, {} ms", r.body_bytes, r.elapsed_ms);
    for (name, value) in &r.shown_headers {
        let _ = writeln!(out, "{name}: {value}");
    }
    let file = r.body_file.as_deref().unwrap_or("not saved");
    if r.binary {
        let _ = writeln!(
            out,
            "(binary body, {} bytes, saved to {file})",
            r.body_bytes
        );
    }
    if let Some(body) = &r.rendered {
        let _ = writeln!(out, "{body}");
    }
    if let Some(text) = &r.body_text {
        out.push_str(text);
        if !text.ends_with('\n') {
            out.push('\n');
        }
        if r.omitted_bytes > 0 {
            let _ = writeln!(
                out,
                "... {} more bytes (full body: {file})",
                r.omitted_bytes
            );
        }
    }
    for s in &r.selected {
        match (s.values.as_slice(), s.omitted) {
            ([], 0) => {
                let _ = writeln!(out, "{}: no match", s.path);
            }
            ([one], 0) => {
                let _ = writeln!(out, "{}: {one}", s.path);
            }
            ([], omitted) => {
                let _ = writeln!(
                    out,
                    "{}: {} not shown, over --budget (full body: {file})",
                    s.path,
                    matches(omitted)
                );
            }
            (values, omitted) => {
                let _ = writeln!(out, "{}: {}", s.path, matches(values.len() + omitted));
                for v in values {
                    let _ = writeln!(out, "  {v}");
                }
                if omitted > 0 {
                    let _ = writeln!(out, "  ... {omitted} more matches (full body: {file})");
                }
            }
        }
    }
    let failed: Vec<&Checked> = r.assertions.iter().filter(|c| !c.passed).collect();
    for c in &failed {
        let reason = c.reason.as_deref().unwrap_or("failed");
        let _ = writeln!(out, "expect failed: {}: {reason}", c.assertion);
    }
    if failed.is_empty() && !r.assertions.is_empty() {
        let _ = writeln!(out, "expect: {} passed", r.assertions.len());
    }
    out
}

fn matches(n: usize) -> String {
    if n == 1 {
        "1 match".into()
    } else {
        format!("{n} matches")
    }
}

/// A URL with a scheme: one without is http:// for this machine and
/// https:// for anywhere else.
fn full_url(given: &str) -> Result<String, String> {
    if given.contains("://") {
        http::check_url(given)?;
        return Ok(given.to_string());
    }
    let authority = given.split(['/', '?', '#']).next().unwrap_or("");
    let host = match authority.strip_prefix('[') {
        Some(v6) => v6.split(']').next().unwrap_or(""),
        None => authority.split(':').next().unwrap_or(""),
    };
    let local = matches!(host, "localhost" | "127.0.0.1" | "::1" | "0.0.0.0")
        || host.ends_with(".localhost");
    let scheme = if local { "http" } else { "https" };
    Ok(format!("{scheme}://{given}"))
}

/// --data or --json-body: inline text (with `${NAME}` expanded), `@FILE`, or
/// `@-` for stdin.
fn body_arg(arg: &str, flag: &str, secrets: &mut Secrets) -> Result<Vec<u8>, Refusal> {
    match arg.strip_prefix('@') {
        Some("-") => {
            let mut bytes = Vec::new();
            std::io::stdin()
                .read_to_end(&mut bytes)
                .map_err(|e| usage(format!("{flag} @-: cannot read stdin: {e}")))?;
            Ok(bytes)
        }
        Some(path) => std::fs::read(path).map_err(|e| {
            let exit = if e.kind() == std::io::ErrorKind::NotFound {
                Exit::NotFound
            } else {
                Exit::Usage
            };
            (exit, format!("{flag} @{path}: {e}"))
        }),
        None => Ok(secrets.expand(arg)?.into_bytes()),
    }
}

/// Whether a header name or JSON key names a credential.
fn secret_name(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    SECRET_HEADERS.contains(&name.as_str()) || SECRET_WORDS.iter().any(|w| name.contains(w))
}

/// Saves the whole (redacted) body where the marker says, pretty-printed
/// when it is JSON so it can be read by line.
fn save_text(text: &str, json: Option<&Value>) -> Option<String> {
    match json {
        Some(v) => save(serde_json::to_string_pretty(v).ok()?.as_bytes(), "json"),
        None => save(text.as_bytes(), "txt"),
    }
}

fn save(bytes: &[u8], ext: &str) -> Option<String> {
    let dir = jouet_output::paths::log_dir();
    std::fs::create_dir_all(&dir).ok()?;
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    let path: PathBuf = dir.join(format!("http-{millis}-{}.{ext}", std::process::id()));
    std::fs::write(&path, bytes).ok()?;
    Some(path.display().to_string())
}

fn floor_char_boundary(text: &str, at: usize) -> usize {
    let mut end = at.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    end
}

/// The secret values this request must never print: what `${NAME}`
/// expanded to, credentials sent in headers, and the values of the fnox
/// project's secrets in the environment.
struct Secrets {
    values: Vec<String>,
}

impl Secrets {
    fn from_fnox() -> Self {
        let mut secrets = Self { values: Vec::new() };
        let cwd = std::env::current_dir().unwrap_or_default();
        if let Some(project) = crate::fnox::Project::find(&cwd) {
            for name in &project.secrets {
                if let Ok(value) = std::env::var(name) {
                    secrets.add(&value);
                }
            }
        }
        secrets
    }

    fn add(&mut self, value: &str) {
        if value.len() >= MIN_SECRET && !self.values.iter().any(|v| v == value) {
            self.values.push(value.to_string());
            // The longest first, so a value inside another is not left
            // half-masked.
            self.values.sort_by_key(|v| std::cmp::Reverse(v.len()));
        }
    }

    /// `${NAME}` replaced by the environment's value, which is then a
    /// secret. An unset name refuses the request.
    fn expand(&mut self, text: &str) -> Result<String, Refusal> {
        let mut out = String::new();
        let mut last = 0;
        for caps in VAR.captures_iter(text) {
            let whole = caps.get(0).expect("a match has group 0");
            let name = &caps[1];
            let value = std::env::var(name).map_err(|_| {
                (
                    Exit::Precondition,
                    format!(
                        "`${{{name}}}` is not set: export it, or run under `fnox exec -- jouet http ...` if fnox holds it"
                    ),
                )
            })?;
            out.push_str(&text[last..whole.start()]);
            out.push_str(&value);
            last = whole.end();
            self.add(&value);
        }
        out.push_str(&text[last..]);
        Ok(out)
    }

    /// The text with every known secret value, and anything shaped like a
    /// token, replaced by `[redacted]`.
    fn hide(&self, text: &str) -> String {
        let mut out = text.to_string();
        for value in &self.values {
            out = out.replace(value.as_str(), MASK);
        }
        redact::redact(&out)
    }

    /// Likewise for every string in a JSON value, and the whole of a string
    /// under a key that names a credential (`access_token`, `password`).
    fn hide_value(&self, value: &mut Value) {
        match value {
            Value::String(s) => *s = self.hide(s),
            Value::Array(items) => items.iter_mut().for_each(|v| self.hide_value(v)),
            Value::Object(map) => {
                for (key, v) in map.iter_mut() {
                    match v {
                        Value::String(s) if secret_name(key) && !s.is_empty() => {
                            *s = MASK.to_string();
                        }
                        v => self.hide_value(v),
                    }
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_url_without_a_scheme_is_http_only_for_this_machine() {
        assert_eq!(
            full_url("localhost:3000/x").unwrap(),
            "http://localhost:3000/x"
        );
        assert_eq!(full_url("127.0.0.1/x").unwrap(), "http://127.0.0.1/x");
        assert_eq!(full_url("[::1]:8080").unwrap(), "http://[::1]:8080");
        assert_eq!(
            full_url("app.localhost/x").unwrap(),
            "http://app.localhost/x"
        );
        assert_eq!(
            full_url("api.github.com/x").unwrap(),
            "https://api.github.com/x"
        );
        assert_eq!(
            full_url("localhost.evil.com").unwrap(),
            "https://localhost.evil.com"
        );
        assert!(full_url("ftp://example.com").is_err());
    }

    #[test]
    fn secrets_are_hidden_wherever_they_appear() {
        let mut s = Secrets { values: Vec::new() };
        s.add("hunter2-long-value");
        s.add("abc");
        assert_eq!(
            s.values,
            ["hunter2-long-value"],
            "short values are not searched for"
        );
        assert_eq!(s.hide("echo: hunter2-long-value!"), "echo: [redacted]!");
        assert_eq!(s.hide("tok ghp_0123456789abcdefghij0123"), "tok [redacted]");
        let mut v = json!({
            "access_token": "anything",
            "tokens_used": 12,
            "nested": [{"note": "hunter2-long-value"}],
            "name": "ok",
        });
        s.hide_value(&mut v);
        assert_eq!(
            v,
            json!({
                "access_token": "[redacted]",
                "tokens_used": 12,
                "nested": [{"note": "[redacted]"}],
                "name": "ok",
            })
        );
        assert!(secret_name("Authorization"));
        assert!(secret_name("x-api-key"));
        assert!(secret_name("set-cookie"));
        assert!(!secret_name("x-ratelimit-remaining"));
        assert!(!secret_name("content-type"));
    }

    #[test]
    fn variables_expand_from_the_environment_and_become_secrets() {
        let mut s = Secrets { values: Vec::new() };
        // PATH is set wherever the tests run.
        let path = std::env::var("PATH").unwrap();
        assert_eq!(s.expand("a ${PATH} b").unwrap(), format!("a {path} b"));
        assert!(s.values.contains(&path));
        assert_eq!(s.expand("$PATH ${ not}").unwrap(), "$PATH ${ not}");
        let (exit, message) = s.expand("${JOUET_TEST_SURELY_UNSET}").unwrap_err();
        assert_eq!(exit, Exit::Precondition);
        assert!(
            message.contains("`${JOUET_TEST_SURELY_UNSET}` is not set"),
            "{message}"
        );
    }

    #[test]
    fn a_long_body_is_cut_at_a_character_boundary() {
        assert_eq!(floor_char_boundary("aé", 2), 1);
        assert_eq!(floor_char_boundary("ab", 10), 2);
    }
}

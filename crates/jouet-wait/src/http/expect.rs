//! hurl-style assertions on a response, and JSONPath (RFC 9535) selection.
//!
//! An assertion is a query, an optional `count`, an optional `not`, and a
//! predicate with its value:
//!
//! ```text
//! status == 200            status=200 is the same
//! header Content-Type contains json
//! jsonpath $.ok == true
//! jsonpath "$.items[?@.state == 'open']" count >= 1
//! jsonpath $.error not exists
//! body matches "^ok"
//! duration < 500           milliseconds
//! ```
//!
//! The predicates are `==` (or `=`), `!=`, `>`, `>=`, `<`, `<=`, `contains`
//! (a substring, or an element of a list), `startsWith`, `endsWith`,
//! `matches` (a regex), `exists` and `isEmpty`; with none, `exists`. A value
//! is JSON when it parses as JSON (`true`, `3`, `"a b"`, `null`), and a bare
//! string otherwise.

use regex::Regex;
use serde::Serialize;
use serde_json::Value;
use serde_json_path::JsonPath;
use std::cmp::Ordering;
use std::time::Duration;

/// What an assertion is checked against.
pub struct Facts<'a> {
    pub status: u16,
    /// Lowercase names.
    pub headers: &'a [(String, String)],
    pub body: &'a [u8],
    /// The body parsed as JSON, when it is JSON.
    pub json: Option<&'a Value>,
    pub elapsed: Duration,
}

/// One assertion, parsed.
pub struct Assertion {
    text: String,
    query: Query,
    count: bool,
    negate: bool,
    predicate: Predicate,
}

enum Query {
    Status,
    /// The lowercase name.
    Header(String),
    JsonPath(JsonPath),
    Body,
    /// In milliseconds.
    Duration,
}

enum Predicate {
    Compare(Ordering, bool, Value),
    NotEqual(Value),
    Contains(Value),
    StartsWith(String),
    EndsWith(String),
    Matches(Regex),
    Exists,
    IsEmpty,
}

/// An assertion's result.
#[derive(Debug, Clone, Serialize)]
pub struct Checked {
    /// As it was given.
    pub assertion: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub passed: bool,
    /// The value the query found; absent when it found none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actual: Option<Value>,
    /// Why it failed, such as `got false` or `no match`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl Assertion {
    pub fn parse(text: &str) -> Result<Self, String> {
        let bad = |why: String| format!("`--expect {text}`: {why}");
        let mut rest = text.trim_start();
        let keyword_end = rest
            .find(|c: char| !c.is_ascii_alphabetic())
            .unwrap_or(rest.len());
        let keyword = &rest[..keyword_end];
        rest = rest[keyword_end..].trim_start();
        let query = match keyword.to_ascii_lowercase().as_str() {
            "status" => Query::Status,
            "body" => Query::Body,
            "duration" => Query::Duration,
            "header" => {
                let (name, after) = operand(rest, false).map_err(bad)?;
                if name.is_empty() {
                    return Err(bad(
                        "`header` needs a name, as in `header Content-Type contains json`".into(),
                    ));
                }
                rest = after;
                Query::Header(name.to_ascii_lowercase())
            }
            "jsonpath" => {
                let (path, after) = operand(rest, true).map_err(bad)?;
                rest = after;
                Query::JsonPath(parse_path(&path).map_err(bad)?)
            }
            _ => {
                return Err(bad(format!(
                    "an assertion starts with status, header NAME, jsonpath PATH, body or duration, not `{keyword}`"
                )));
            }
        };
        let mut count = false;
        if let Some(after) = word(rest, "count") {
            count = true;
            rest = after;
        }
        let mut negate = false;
        if let Some(after) = word(rest, "not") {
            negate = true;
            rest = after;
        }
        let predicate = predicate(rest).map_err(bad)?;
        Ok(Self {
            text: text.trim().to_string(),
            query,
            count,
            negate,
            predicate,
        })
    }

    /// Whether it asks about the status, which `jouet http` otherwise
    /// requires to be below 400.
    pub fn is_status(&self) -> bool {
        matches!(self.query, Query::Status)
    }

    /// Whether checking it needs the body.
    pub fn needs_body(&self) -> bool {
        matches!(self.query, Query::JsonPath(_) | Query::Body)
    }

    pub fn check(&self, facts: &Facts<'_>) -> Checked {
        let fail = |actual: Option<Value>, reason: String| Checked {
            assertion: self.text.clone(),
            passed: false,
            actual,
            reason: Some(reason),
        };
        // A header sent more than once passes if any of its values does.
        let candidates: Vec<Value> = match &self.query {
            Query::Status => vec![facts.status.into()],
            Query::Body => vec![String::from_utf8_lossy(facts.body).into_owned().into()],
            Query::Duration => {
                vec![
                    u64::try_from(facts.elapsed.as_millis())
                        .unwrap_or(u64::MAX)
                        .into(),
                ]
            }
            Query::Header(name) => {
                let values: Vec<Value> = facts
                    .headers
                    .iter()
                    .filter(|(n, _)| n == name)
                    .map(|(_, v)| v.clone().into())
                    .collect();
                if self.count {
                    vec![values.len().into()]
                } else {
                    values
                }
            }
            Query::JsonPath(path) => {
                let Some(json) = facts.json else {
                    return fail(None, "the body is not JSON".into());
                };
                let nodes = path.query(json).all();
                if self.count {
                    // As in hurl, `$.items count` counts the list's items.
                    match nodes.as_slice() {
                        [Value::Array(items)] => vec![items.len().into()],
                        _ => vec![nodes.len().into()],
                    }
                } else {
                    match nodes.as_slice() {
                        [] => Vec::new(),
                        [one] => vec![(*one).clone()],
                        many => vec![Value::Array(many.iter().map(|v| (*v).clone()).collect())],
                    }
                }
            }
        };
        let exists = !candidates.is_empty();
        let passed = match &self.predicate {
            Predicate::Exists => exists != self.negate,
            _ if !exists => return fail(None, "no match".into()),
            predicate => candidates
                .iter()
                .any(|actual| predicate.holds(actual) != self.negate),
        };
        let actual = match candidates.len() {
            0 => None,
            1 => candidates.into_iter().next(),
            _ => Some(Value::Array(candidates)),
        };
        if passed {
            Checked {
                assertion: self.text.clone(),
                passed,
                actual,
                reason: None,
            }
        } else {
            let reason = match &actual {
                Some(v) => format!("got {}", short(v)),
                None => "no match".into(),
            };
            fail(actual, reason)
        }
    }
}

impl Predicate {
    fn holds(&self, actual: &Value) -> bool {
        match self {
            Self::Compare(Ordering::Equal, _, want) => same(actual, want),
            Self::Compare(ord, or_equal, want) => compare(actual, want)
                .is_some_and(|o| o == *ord || (*or_equal && o == Ordering::Equal)),
            Self::NotEqual(want) => !same(actual, want),
            Self::Contains(want) => match (actual, want) {
                (Value::String(s), Value::String(w)) => s.contains(w.as_str()),
                (Value::Array(items), want) => items.iter().any(|i| same(i, want)),
                (Value::Object(map), Value::String(key)) => map.contains_key(key),
                _ => false,
            },
            Self::StartsWith(w) => actual.as_str().is_some_and(|s| s.starts_with(w.as_str())),
            Self::EndsWith(w) => actual.as_str().is_some_and(|s| s.ends_with(w.as_str())),
            Self::Matches(re) => match actual {
                Value::String(s) => re.is_match(s),
                Value::Number(n) => re.is_match(&n.to_string()),
                _ => false,
            },
            Self::IsEmpty => match actual {
                Value::String(s) => s.is_empty(),
                Value::Array(a) => a.is_empty(),
                Value::Object(o) => o.is_empty(),
                _ => false,
            },
            Self::Exists => true,
        }
    }
}

/// A JSONPath for `--select`.
pub fn select(path: &str) -> Result<JsonPath, String> {
    parse_path(path).map_err(|e| format!("`--select {path}`: {e}"))
}

fn parse_path(path: &str) -> Result<JsonPath, String> {
    JsonPath::parse(path).map_err(|e| format!("`{path}` is not a JSONPath: {e}"))
}

/// `rest` starting with `word` and a space (or the end), what follows it.
fn word<'a>(rest: &'a str, word: &str) -> Option<&'a str> {
    let after = rest.strip_prefix(word)?;
    (after.is_empty() || after.starts_with(char::is_whitespace)).then(|| after.trim_start())
}

/// A header name or a JSONPath: quoted, or up to the next space (for a
/// path, a space outside brackets; for a name, also an operator).
fn operand(rest: &str, path: bool) -> Result<(String, &str), String> {
    if rest.starts_with('"') || rest.starts_with('\'') {
        let (value, after) = quoted(rest)?;
        return Ok((value, after.trim_start()));
    }
    let mut depth = 0usize;
    let mut quote = None;
    let mut end = rest.len();
    for (i, c) in rest.char_indices() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '\'' | '"') if depth > 0 => quote = Some(c),
            (None, '[' | '(') if path => depth += 1,
            (None, ']' | ')') if path => depth = depth.saturating_sub(1),
            (None, c) if depth == 0 && c.is_whitespace() => {
                end = i;
                break;
            }
            (None, '=' | '!' | '<' | '>') if !path => {
                end = i;
                break;
            }
            _ => {}
        }
    }
    Ok((rest[..end].to_string(), rest[end..].trim_start()))
}

/// A `"double"` (JSON escapes) or `'single'` quoted string at the start of
/// `rest`, and what follows it.
fn quoted(rest: &str) -> Result<(String, &str), String> {
    if let Some(inner) = rest.strip_prefix('\'') {
        let end = inner
            .find('\'')
            .ok_or_else(|| "a ' quote is not closed".to_string())?;
        return Ok((inner[..end].to_string(), &inner[end + 1..]));
    }
    let mut escaped = false;
    for (i, c) in rest.char_indices().skip(1) {
        match c {
            _ if escaped => escaped = false,
            '\\' => escaped = true,
            '"' => {
                let value: String = serde_json::from_str(&rest[..=i])
                    .map_err(|e| format!("a quoted string is not valid: {e}"))?;
                return Ok((value, &rest[i + 1..]));
            }
            _ => {}
        }
    }
    Err("a \" quote is not closed".into())
}

fn predicate(rest: &str) -> Result<Predicate, String> {
    let rest = rest.trim();
    if rest.is_empty() {
        return Ok(Predicate::Exists);
    }
    const OPS: &[&str] = &["==", "!=", ">=", "<=", ">", "<", "="];
    if let Some(op) = OPS.iter().find(|op| rest.starts_with(**op)) {
        let want = value(&rest[op.len()..])?;
        return Ok(match *op {
            "==" | "=" => Predicate::Compare(Ordering::Equal, true, want),
            "!=" => Predicate::NotEqual(want),
            ">" => Predicate::Compare(Ordering::Greater, false, want),
            ">=" => Predicate::Compare(Ordering::Greater, true, want),
            "<" => Predicate::Compare(Ordering::Less, false, want),
            _ => Predicate::Compare(Ordering::Less, true, want),
        });
    }
    let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
    let (name, arg) = (&rest[..end], &rest[end..]);
    // A number or `true` is taken as written: `endsWith 0.1`.
    let string = || match value(arg)? {
        Value::String(s) => Ok::<_, String>(s),
        _ => Ok(arg.trim().to_string()),
    };
    let predicate = match name {
        "contains" | "includes" => Predicate::Contains(value(arg)?),
        "startsWith" => Predicate::StartsWith(string()?),
        "endsWith" => Predicate::EndsWith(string()?),
        "matches" => {
            let re = string()?;
            Predicate::Matches(Regex::new(&re).map_err(|e| format!("`{re}` is not a regex: {e}"))?)
        }
        "exists" | "isEmpty" if !arg.trim().is_empty() => {
            return Err(format!("`{name}` takes no value"));
        }
        "exists" => Predicate::Exists,
        "isEmpty" => Predicate::IsEmpty,
        _ => {
            return Err(format!(
                "`{name}` is not a predicate: use ==, !=, >, >=, <, <=, contains, startsWith, endsWith, matches, exists or isEmpty"
            ));
        }
    };
    Ok(predicate)
}

/// A predicate's value: JSON when it parses as JSON, else the text, with
/// single quotes taken off.
fn value(text: &str) -> Result<Value, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("the predicate needs a value".into());
    }
    if let Ok(v) = serde_json::from_str(text) {
        return Ok(v);
    }
    if text.starts_with('\'') || text.starts_with('"') {
        let (s, after) = quoted(text)?;
        if after.trim().is_empty() {
            return Ok(Value::String(s));
        }
    }
    Ok(Value::String(text.to_string()))
}

/// Equal as JSON, with numbers compared by value (`1 == 1.0`).
fn same(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x.as_f64() == y.as_f64(),
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(x, y)| same(x, y))
        }
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| same(v, w)))
        }
        _ => a == b,
    }
}

/// Numbers by value, strings in byte order; anything else does not compare.
fn compare(a: &Value, b: &Value) -> Option<Ordering> {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x.as_f64()?.partial_cmp(&y.as_f64()?),
        (Value::String(x), Value::String(y)) => Some(x.cmp(y)),
        _ => None,
    }
}

/// A value for a failure message, cut to a line.
fn short(v: &Value) -> String {
    const MAX: usize = 200;
    let text = v.to_string();
    if text.len() <= MAX {
        return text;
    }
    let mut end = MAX;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}... ({} bytes)", &text[..end], text.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn facts<'a>(json: &'a Value, headers: &'a [(String, String)]) -> Facts<'a> {
        Facts {
            status: 200,
            headers,
            body: b"{\"ok\":true}",
            json: Some(json),
            elapsed: Duration::from_millis(42),
        }
    }

    fn check(assertion: &str, json: &Value) -> Checked {
        let headers = vec![
            (
                "content-type".to_string(),
                "application/json; charset=utf-8".to_string(),
            ),
            ("set-cookie".to_string(), "a=1".to_string()),
            ("set-cookie".to_string(), "b=2".to_string()),
        ];
        Assertion::parse(assertion)
            .unwrap_or_else(|e| panic!("{e}"))
            .check(&facts(json, &headers))
    }

    #[test]
    fn hurl_style_assertions_pass_and_fail_as_written() {
        let doc = json!({
            "ok": true,
            "count": 3,
            "name": "jouet 0.1",
            "items": [{"id": 1, "state": "open"}, {"id": 2, "state": "closed"}],
            "empty": [],
        });
        let passing = [
            "status == 200",
            "status=200",
            "status >= 200",
            "status < 300",
            "status matches ^2",
            "status != 404",
            "header Content-Type contains json",
            "header \"content-type\" startsWith application/json",
            "header set-cookie == b=2",
            "header set-cookie count == 2",
            "header x-missing not exists",
            "jsonpath $.ok == true",
            "jsonpath $.ok",
            "jsonpath $.count = 3.0",
            "jsonpath $.count > 2",
            "jsonpath $.name == 'jouet 0.1'",
            "jsonpath $.name == \"jouet 0.1\"",
            "jsonpath $.name endsWith 0.1",
            "jsonpath $.items[*].id == [1, 2]",
            "jsonpath $.items[*].id contains 2",
            "jsonpath $.items[?@.state == 'open'].id == 1",
            "jsonpath \"$.items[?@.state == 'open']\" count == 1",
            "jsonpath $.nothing count == 0",
            "jsonpath $.items count == 2",
            "jsonpath $.items[*] count == 2",
            "jsonpath $.error not exists",
            "jsonpath $.empty isEmpty",
            "body contains ok",
            "duration < 1000",
        ];
        for a in passing {
            let c = check(a, &doc);
            assert!(c.passed, "{a}: {c:?}");
        }
        let failing = [
            ("status == 201", Some("got 200")),
            ("jsonpath $.ok == false", Some("got true")),
            ("jsonpath $.missing == 1", Some("no match")),
            ("jsonpath $.missing != 1", Some("no match")),
            ("jsonpath $.ok not exists", Some("got true")),
            ("jsonpath $.name > 3", Some("got \"jouet 0.1\"")),
            ("header x-missing exists", Some("no match")),
            ("header set-cookie == c=3", Some("got [\"a=1\",\"b=2\"]")),
        ];
        for (a, reason) in failing {
            let c = check(a, &doc);
            assert!(!c.passed, "{a}: {c:?}");
            assert_eq!(c.reason.as_deref(), reason, "{a}");
        }
    }

    #[test]
    fn a_body_that_is_not_json_fails_a_jsonpath() {
        let c = Assertion::parse("jsonpath $.ok").unwrap().check(&Facts {
            json: None,
            ..facts(&Value::Null, &[])
        });
        assert!(!c.passed);
        assert_eq!(c.reason.as_deref(), Some("the body is not JSON"));
    }

    #[test]
    fn bad_assertions_say_what_is_wrong() {
        for (a, says) in [
            ("stauts == 200", "starts with status, header NAME"),
            ("header == 1", "`header` needs a name"),
            ("jsonpath $..[ == 1", "is not a JSONPath"),
            ("status ~ 200", "is not a predicate"),
            ("status ==", "needs a value"),
            ("jsonpath $.a matches (", "is not a regex"),
            ("jsonpath $.a exists 3", "takes no value"),
            ("header 'x == 1", "quote is not closed"),
        ] {
            let e = Assertion::parse(a)
                .err()
                .unwrap_or_else(|| panic!("{a} parsed"));
            assert!(e.contains(says), "{a}: {e}");
            assert!(e.starts_with(&format!("`--expect {a}`")), "{e}");
        }
    }

    #[test]
    fn only_status_and_body_queries_say_so() {
        assert!(Assertion::parse("status=200").unwrap().is_status());
        assert!(!Assertion::parse("header a").unwrap().is_status());
        assert!(Assertion::parse("jsonpath $.a").unwrap().needs_body());
        assert!(Assertion::parse("body contains a").unwrap().needs_body());
        assert!(!Assertion::parse("header a").unwrap().needs_body());
    }

    #[test]
    fn selection_takes_a_jsonpath() {
        let path = select("$.items[*].id").unwrap();
        let doc = json!({"items": [{"id": 1}, {"id": 2}]});
        assert_eq!(path.query(&doc).all(), [&json!(1), &json!(2)]);
        assert!(select("items").unwrap_err().starts_with("`--select items`"));
    }
}

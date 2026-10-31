//! gitleaks' rule format, and finding secrets in text with it.
//!
//! A rule has an `id`, a `regex`, `keywords` of which one must appear
//! (case-insensitively) before the regex is tried, an optional
//! `secretGroup` naming the part of a match that is the secret (else the
//! first group that matched anything, else the whole match), an optional
//! `entropy` the secret must exceed, an optional `path` the file must
//! match, and allowlists. A config's own allowlists apply to every rule,
//! or to the rules their `targetRules` name.
//!
//! An allowlist lets a finding through when any of its `paths`, `regexes`,
//! `stopwords` or `commits` matches (`condition = "AND"`: when all of the
//! kinds it has match). Its regexes test the secret, or with `regexTarget`
//! the whole match or the line. Stopwords are found anywhere in the secret,
//! ignoring case.
//!
//! Regexes are Go's RE2 syntax, which the `regex` crate reads without
//! Unicode: `\w`, `\d`, `\s` and `\b` are ASCII, as in Go. Each is compiled
//! the first time a fragment holds one of its rule's keywords, so a scan
//! pays only for the rules its text could match.

use aho_corasick::AhoCorasick;
use regex::bytes::{Regex, RegexBuilder, RegexSet, RegexSetBuilder};
use serde::Deserialize;
use std::sync::OnceLock;

/// gitleaks' default rules, vendored unchanged (see `gitleaks/` and
/// `mise run secrets:update`).
pub(crate) const DEFAULT: &str = include_str!("../gitleaks/gitleaks.toml");

/// The gitleaks release the default rules come from.
pub(crate) const VERSION: &str = include_str!("../gitleaks/VERSION");

/// What marks a line as a known false positive, as in gitleaks.
const INLINE_ALLOW: &[u8] = b"gitleaks:allow";

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct ConfigDecl {
    #[serde(default)]
    rules: Vec<RuleDecl>,
    /// The older, single form of `allowlists`.
    allowlist: Option<AllowDecl>,
    #[serde(default)]
    allowlists: Vec<AllowDecl>,
    extend: Option<ExtendDecl>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RuleDecl {
    id: String,
    #[serde(default)]
    description: String,
    regex: Option<String>,
    path: Option<String>,
    #[serde(default)]
    secret_group: usize,
    entropy: Option<f64>,
    #[serde(default)]
    keywords: Vec<String>,
    allowlist: Option<AllowDecl>,
    #[serde(default)]
    allowlists: Vec<AllowDecl>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct AllowDecl {
    condition: Option<String>,
    #[serde(default)]
    commits: Vec<String>,
    #[serde(default)]
    paths: Vec<String>,
    #[serde(default)]
    regexes: Vec<String>,
    regex_target: Option<String>,
    #[serde(default)]
    stopwords: Vec<String>,
    #[serde(default)]
    target_rules: Vec<String>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct ExtendDecl {
    #[serde(default)]
    use_default: bool,
    #[serde(default)]
    disabled_rules: Vec<String>,
}

/// An allowlist's `paths` and `regexes`, compiled; `None` when it has none.
type AllowSets = (Option<RegexSet>, Option<RegexSet>);

/// A rule's `regex` and `path`, compiled.
type RuleRegexes = (Option<Regex>, Option<Regex>);

/// What an allowlist's regexes test.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    Secret,
    Match,
    Line,
}

#[derive(Debug)]
struct Allowlist {
    all: bool,
    commits: Vec<String>,
    paths: Vec<String>,
    regexes: Vec<String>,
    target: Target,
    stopwords: Vec<String>,
    /// The rules a config-wide allowlist is for; empty for all of them.
    target_rules: Vec<String>,
    compiled: OnceLock<Result<AllowSets, String>>,
}

/// What one candidate looks like to an allowlist.
pub(crate) struct Candidate<'a> {
    pub secret: &'a [u8],
    pub matched: &'a [u8],
    pub line: &'a [u8],
    pub path: &'a str,
    pub commit: Option<&'a str>,
}

impl Allowlist {
    fn new(decl: AllowDecl) -> Result<Self, String> {
        let target = match decl.regex_target.as_deref() {
            None | Some("" | "secret") => Target::Secret,
            Some("match") => Target::Match,
            Some("line") => Target::Line,
            Some(other) => return Err(format!("unknown regexTarget `{other}`")),
        };
        let all = match decl.condition.as_deref().map(str::to_ascii_uppercase) {
            None => false,
            Some(c) if c.is_empty() || c == "OR" => false,
            Some(c) if c == "AND" => true,
            Some(c) => return Err(format!("unknown allowlist condition `{c}`")),
        };
        Ok(Self {
            all,
            commits: decl.commits,
            paths: decl.paths,
            regexes: decl.regexes,
            target,
            stopwords: decl.stopwords.iter().map(|s| s.to_lowercase()).collect(),
            target_rules: decl.target_rules,
            compiled: OnceLock::new(),
        })
    }

    fn sets(&self) -> Result<&AllowSets, String> {
        self.compiled
            .get_or_init(|| Ok((set(&self.paths)?, set(&self.regexes)?)))
            .as_ref()
            .map_err(Clone::clone)
    }

    fn path_allowed(&self, path: &str) -> Result<bool, String> {
        Ok(self
            .sets()?
            .0
            .as_ref()
            .is_some_and(|s| s.is_match(path.as_bytes())))
    }

    fn allows(&self, c: &Candidate<'_>) -> Result<bool, String> {
        let (paths, regexes) = self.sets()?;
        let commit = || {
            c.commit
                .is_some_and(|commit| self.commits.iter().any(|a| commit.starts_with(a.as_str())))
        };
        let path = || {
            paths
                .as_ref()
                .is_some_and(|s| s.is_match(c.path.as_bytes()))
        };
        let regex = || {
            regexes.as_ref().is_some_and(|s| {
                s.is_match(match self.target {
                    Target::Secret => c.secret,
                    Target::Match => c.matched,
                    Target::Line => c.line,
                })
            })
        };
        let stopword = || {
            let secret = String::from_utf8_lossy(c.secret).to_lowercase();
            self.stopwords.iter().any(|w| secret.contains(w.as_str()))
        };
        if self.all {
            let checks: [(bool, &dyn Fn() -> bool); 4] = [
                (!self.commits.is_empty(), &commit),
                (paths.is_some(), &path),
                (regexes.is_some(), &regex),
                (!self.stopwords.is_empty(), &stopword),
            ];
            let mut any = false;
            for (present, check) in checks {
                if present {
                    any = true;
                    if !check() {
                        return Ok(false);
                    }
                }
            }
            return Ok(any);
        }
        Ok(commit() || path() || regex() || stopword())
    }
}

/// gitleaks compiles a pattern once and keeps it; so does this, the first
/// time it is needed.
fn compile(pattern: &str) -> Result<Regex, String> {
    RegexBuilder::new(&from_go(pattern))
        .unicode(false)
        .size_limit(64 << 20)
        .build()
        .map_err(|e| e.to_string())
}

fn set(patterns: &[String]) -> Result<Option<RegexSet>, String> {
    if patterns.is_empty() {
        return Ok(None);
    }
    RegexSetBuilder::new(patterns.iter().map(|p| from_go(p)))
        .unicode(false)
        .size_limit(64 << 20)
        .build()
        .map(Some)
        .map_err(|e| e.to_string())
}

/// A Go regex as the `regex` crate reads the same: Go takes a `{` that
/// starts no repetition (`{{ .x }}`, `{\d+}`) and its `}` literally, and a
/// `[`, `&` or `~` inside a class too, where the `regex` crate would read a
/// nested class or a set operation.
pub(crate) fn from_go(pattern: &str) -> String {
    let chars: Vec<char> = pattern.chars().collect();
    let mut out = String::with_capacity(pattern.len() + 8);
    let mut i = 0;
    let mut class = false;
    // Whether the `}`s still to come close repetitions.
    let mut repeats = 0usize;
    while i < chars.len() {
        let c = chars[i];
        if c == '\\' {
            out.push(c);
            if let Some(next) = chars.get(i + 1) {
                out.push(*next);
            }
            i += 2;
            continue;
        }
        if class {
            match c {
                ']' => class = false,
                '[' if chars.get(i + 1) == Some(&':') => {
                    // A POSIX class, `[:alnum:]`, kept whole.
                    let end = (i + 2..chars.len().saturating_sub(1))
                        .find(|&j| chars[j] == ':' && chars[j + 1] == ']');
                    if let Some(end) = end {
                        out.extend(&chars[i..=end + 1]);
                        i = end + 2;
                        continue;
                    }
                    out.push('\\');
                }
                '[' | '&' | '~' => out.push('\\'),
                _ => {}
            }
            out.push(c);
            i += 1;
            continue;
        }
        match c {
            '[' => {
                class = true;
                out.push(c);
                i += 1;
                // A `]` first in a class, or after `^`, is literal.
                if chars.get(i) == Some(&'^') {
                    out.push('^');
                    i += 1;
                }
                if chars.get(i) == Some(&']') {
                    out.push_str("\\]");
                    i += 1;
                }
                continue;
            }
            '{' if repetition(&chars[i + 1..]) => repeats += 1,
            '{' => out.push('\\'),
            '}' if repeats > 0 => repeats -= 1,
            '}' => out.push('\\'),
            _ => {}
        }
        out.push(c);
        i += 1;
    }
    out
}

/// Whether what follows a `{` makes it a repetition: `n}`, `n,}` or
/// `n,m}`.
fn repetition(rest: &[char]) -> bool {
    let digits = |s: &[char]| s.iter().take_while(|c| c.is_ascii_digit()).count();
    let n = digits(rest);
    if n == 0 {
        return false;
    }
    match rest.get(n) {
        Some('}') => true,
        Some(',') => {
            let m = digits(&rest[n + 1..]);
            rest.get(n + 1 + m) == Some(&'}')
        }
        _ => false,
    }
}

#[derive(Debug)]
struct Rule {
    id: String,
    description: String,
    regex: Option<String>,
    path: Option<String>,
    secret_group: usize,
    entropy: Option<f64>,
    keywords: Vec<String>,
    allowlists: Vec<Allowlist>,
    compiled: OnceLock<Result<RuleRegexes, String>>,
}

impl Rule {
    fn new(decl: RuleDecl) -> Result<Self, String> {
        let mut allowlists = Vec::new();
        for a in decl.allowlist.into_iter().chain(decl.allowlists) {
            allowlists.push(Allowlist::new(a).map_err(|e| format!("rule {}: {e}", decl.id))?);
        }
        Ok(Self {
            id: decl.id,
            description: decl.description,
            regex: decl.regex.filter(|r| !r.is_empty()),
            path: decl.path.filter(|p| !p.is_empty()),
            secret_group: decl.secret_group,
            entropy: decl.entropy.filter(|e| *e > 0.0),
            keywords: decl.keywords.iter().map(|k| k.to_lowercase()).collect(),
            allowlists,
            compiled: OnceLock::new(),
        })
    }

    fn compiled(&self) -> Result<&RuleRegexes, String> {
        self.compiled
            .get_or_init(|| {
                let regex = self.regex.as_deref().map(compile).transpose();
                let path = self.path.as_deref().map(compile).transpose();
                Ok((regex?, path?))
            })
            .as_ref()
            .map_err(|e| format!("rule {}: {e}", self.id))
    }

    /// Merges an extending config's rule of the same id into this one, as
    /// gitleaks does: its fields win where it sets them, and its
    /// allowlists are added.
    fn extend(&mut self, other: Self) {
        if !other.description.is_empty() {
            self.description = other.description;
        }
        if other.regex.is_some() {
            self.regex = other.regex;
        }
        if other.path.is_some() {
            self.path = other.path;
        }
        if other.secret_group != 0 {
            self.secret_group = other.secret_group;
        }
        if other.entropy.is_some() {
            self.entropy = other.entropy;
        }
        if !other.keywords.is_empty() {
            self.keywords = other.keywords;
        }
        self.allowlists.extend(other.allowlists);
        self.compiled = OnceLock::new();
    }
}

/// A finding before it is reported: the rule, where it is, and the redacted
/// match. The secret itself never leaves this module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Hit {
    pub rule: String,
    pub description: String,
    /// 1-based, within the fragment's text.
    pub line: u64,
    pub redacted: String,
    /// Where the secret is in the fragment.
    span: (usize, usize),
}

/// A set of rules and the allowlists that go with them.
#[derive(Debug)]
pub struct Rules {
    rules: Vec<Rule>,
    allowlists: Vec<Allowlist>,
    /// Finds the rules' keywords in a fragment.
    keywords: AhoCorasick,
    /// For each keyword, the rules that have it.
    keyword_rules: Vec<Vec<usize>>,
    /// Rules with no keywords, tried on every fragment.
    always: Vec<usize>,
}

impl Rules {
    /// gitleaks' default rules.
    pub fn default_rules() -> Self {
        Self::parse(DEFAULT).expect("the vendored gitleaks rules parse")
    }

    /// The rules a config file describes, alone.
    pub fn parse(text: &str) -> Result<Self, String> {
        let decl: ConfigDecl = toml::from_str(text).map_err(|e| e.to_string())?;
        Self::build(decl)
    }

    /// The default rules, extended or replaced by a repository's
    /// `.gitleaks.toml`. It adds to them when it sets `[extend] useDefault =
    /// true`, and here also when it has no rules of its own, only
    /// allowlists; otherwise its rules replace them, as in gitleaks.
    pub fn with_repo_config(text: &str) -> Result<Self, String> {
        let decl: ConfigDecl = toml::from_str(text).map_err(|e| e.to_string())?;
        let extend = decl.extend.as_ref();
        let extends = extend.is_some_and(|e| e.use_default) || decl.rules.is_empty();
        if !extends {
            return Self::build(decl);
        }
        let disabled = extend.map(|e| e.disabled_rules.clone()).unwrap_or_default();
        let mut base: ConfigDecl = toml::from_str(DEFAULT).map_err(|e| e.to_string())?;
        base.rules.retain(|r| !disabled.contains(&r.id));
        let mut rules = Vec::new();
        for r in base.rules {
            rules.push(Rule::new(r)?);
        }
        for r in decl.rules {
            let r = Rule::new(r)?;
            match rules.iter_mut().find(|have| have.id == r.id) {
                Some(have) => have.extend(r),
                None => rules.push(r),
            }
        }
        let allowlists = base
            .allowlist
            .into_iter()
            .chain(base.allowlists)
            .chain(decl.allowlist)
            .chain(decl.allowlists)
            .map(Allowlist::new)
            .collect::<Result<_, _>>()?;
        Self::assemble(rules, allowlists)
    }

    fn build(decl: ConfigDecl) -> Result<Self, String> {
        let rules = decl
            .rules
            .into_iter()
            .map(Rule::new)
            .collect::<Result<_, _>>()?;
        let allowlists = decl
            .allowlist
            .into_iter()
            .chain(decl.allowlists)
            .map(Allowlist::new)
            .collect::<Result<_, _>>()?;
        Self::assemble(rules, allowlists)
    }

    fn assemble(rules: Vec<Rule>, allowlists: Vec<Allowlist>) -> Result<Self, String> {
        let mut words: Vec<String> = Vec::new();
        let mut keyword_rules: Vec<Vec<usize>> = Vec::new();
        let mut always = Vec::new();
        for (i, rule) in rules.iter().enumerate() {
            if rule.regex.is_none() {
                continue;
            }
            if rule.keywords.is_empty() {
                always.push(i);
            }
            for k in &rule.keywords {
                match words.iter().position(|w| w == k) {
                    Some(at) => keyword_rules[at].push(i),
                    None => {
                        words.push(k.clone());
                        keyword_rules.push(vec![i]);
                    }
                }
            }
        }
        let keywords = AhoCorasick::builder()
            .ascii_case_insensitive(true)
            .build(&words)
            .map_err(|e| e.to_string())?;
        Ok(Self {
            rules,
            allowlists,
            keywords,
            keyword_rules,
            always,
        })
    }

    pub fn len(&self) -> usize {
        self.rules.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// Whether the config's allowlists leave `path` out entirely, as
    /// gitleaks' global path allowlist does (lockfiles, images, vendored
    /// code).
    pub(crate) fn path_allowed(&self, path: &str) -> Result<bool, String> {
        for a in self.allowlists.iter().filter(|a| a.target_rules.is_empty()) {
            if !a.all && a.path_allowed(path)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// The rules that match `path` alone, such as a PKCS #12 file.
    pub(crate) fn path_hits(&self, path: &str) -> Result<Vec<Hit>, String> {
        let mut hits = Vec::new();
        for rule in self.rules.iter().filter(|r| r.regex.is_none()) {
            let (_, Some(path_re)) = rule.compiled()? else {
                continue;
            };
            if path_re.is_match(path.as_bytes()) {
                hits.push(Hit {
                    rule: rule.id.clone(),
                    description: rule.description.clone(),
                    line: 0,
                    redacted: format!("file {path}"),
                    span: (0, 0),
                });
            }
        }
        Ok(hits)
    }

    /// The secrets in `text`, a file's content or the lines a diff adds to
    /// it, at `path`. `commit` is the commit that added them, if any.
    pub(crate) fn scan(
        &self,
        text: &[u8],
        path: &str,
        commit: Option<&str>,
    ) -> Result<Vec<Hit>, String> {
        let mut candidates = vec![false; self.rules.len()];
        for &i in &self.always {
            candidates[i] = true;
        }
        for m in self.keywords.find_overlapping_iter(text) {
            for &i in &self.keyword_rules[m.pattern().as_usize()] {
                candidates[i] = true;
            }
        }
        let mut hits = Vec::new();
        for (i, rule) in self.rules.iter().enumerate() {
            if candidates[i] {
                self.scan_rule(rule, text, path, commit, &mut hits)?;
            }
        }
        // A generic rule's match of a secret a specific rule also found
        // says nothing more; gitleaks drops it too.
        let specific: Vec<(usize, usize)> = hits
            .iter()
            .filter(|h| !h.rule.starts_with("generic"))
            .map(|h| h.span)
            .collect();
        hits.retain(|h| {
            !h.rule.starts_with("generic")
                || !specific.iter().any(|s| s.0 < h.span.1 && h.span.0 < s.1)
        });
        hits.sort_by_key(|h| h.line);
        Ok(hits)
    }

    fn scan_rule(
        &self,
        rule: &Rule,
        text: &[u8],
        path: &str,
        commit: Option<&str>,
        hits: &mut Vec<Hit>,
    ) -> Result<(), String> {
        let (Some(regex), path_re) = rule.compiled()? else {
            return Ok(());
        };
        if path_re
            .as_ref()
            .is_some_and(|p| !p.is_match(path.as_bytes()))
        {
            return Ok(());
        }
        for caps in regex.captures_iter(text) {
            let whole = caps.get(0).expect("a match has group 0");
            let secret = if rule.secret_group > 0 {
                match caps.get(rule.secret_group) {
                    Some(m) => m,
                    None => continue,
                }
            } else {
                caps.iter()
                    .skip(1)
                    .flatten()
                    .find(|m| !m.is_empty())
                    .unwrap_or(whole)
            };
            if secret.is_empty() {
                continue;
            }
            let line = line_around(text, whole.start(), whole.end());
            if contains(line, INLINE_ALLOW) {
                continue;
            }
            if let Some(min) = rule.entropy {
                if entropy(secret.as_bytes()) <= min {
                    continue;
                }
                // gitleaks' generic rules also want a digit in the secret,
                // which RE2 cannot say without lookahead.
                if rule.id.starts_with("generic")
                    && !secret.as_bytes().iter().any(u8::is_ascii_digit)
                {
                    continue;
                }
            }
            let candidate = Candidate {
                secret: secret.as_bytes(),
                matched: whole.as_bytes(),
                line,
                path,
                commit,
            };
            if self.allowed(rule, &candidate)? {
                continue;
            }
            hits.push(Hit {
                rule: rule.id.clone(),
                description: rule.description.clone(),
                line: line_of(text, secret.start()),
                redacted: redact(&text[whole.start()..secret.start()], secret.as_bytes()),
                span: (secret.start(), secret.end()),
            });
        }
        Ok(())
    }

    fn allowed(&self, rule: &Rule, c: &Candidate<'_>) -> Result<bool, String> {
        let global = self
            .allowlists
            .iter()
            .filter(|a| a.target_rules.is_empty() || a.target_rules.contains(&rule.id));
        for a in global.chain(&rule.allowlists) {
            if a.allows(c)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Compiles every rule now, for a test that they all compile.
    #[cfg(test)]
    pub(crate) fn compile_all(&self) -> Vec<String> {
        let mut errors = Vec::new();
        for rule in &self.rules {
            if let Err(e) = rule.compiled() {
                errors.push(e);
            }
            for a in &rule.allowlists {
                if let Err(e) = a.sets() {
                    errors.push(format!("rule {}: allowlist: {e}", rule.id));
                }
            }
        }
        for a in &self.allowlists {
            if let Err(e) = a.sets() {
                errors.push(format!("allowlist: {e}"));
            }
        }
        errors
    }
}

/// The 1-based line `at` is on.
fn line_of(text: &[u8], at: usize) -> u64 {
    1 + text[..at].iter().filter(|b| **b == b'\n').count() as u64
}

/// The lines a match spans, whole.
fn line_around(text: &[u8], start: usize, end: usize) -> &[u8] {
    let from = text[..start]
        .iter()
        .rposition(|b| *b == b'\n')
        .map_or(0, |i| i + 1);
    let last = end.saturating_sub(1).max(start);
    let to = text[last..]
        .iter()
        .position(|b| *b == b'\n')
        .map_or(text.len(), |i| last + i);
    &text[from..to.max(from)]
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// Shannon entropy in bits per byte, as gitleaks computes it.
pub(crate) fn entropy(data: &[u8]) -> f64 {
    if data.is_empty() {
        return 0.0;
    }
    let mut counts = [0usize; 256];
    for b in data {
        counts[*b as usize] += 1;
    }
    let len = data.len() as f64;
    counts
        .iter()
        .filter(|c| **c > 0)
        .map(|c| {
            let p = *c as f64 / len;
            -p * p.log2()
        })
        .sum()
}

/// Longest part of a match before its secret that is shown.
const CONTEXT: usize = 40;

/// What is shown of a match: up to [`CONTEXT`] characters of what comes
/// before the secret on its line (`api_key = "`), the first four
/// characters of a secret of 16 or more (a token's prefix, such as
/// `ghp_`), and `****`. Never the rest of the secret, or what follows it.
pub(crate) fn redact(before: &[u8], secret: &[u8]) -> String {
    let before = String::from_utf8_lossy(before);
    let before = before.rsplit('\n').next().unwrap_or_default();
    let mut collapsed = String::new();
    for c in before.trim_start().chars() {
        if !c.is_whitespace() {
            collapsed.push(c);
        } else if !collapsed.ends_with(' ') {
            collapsed.push(' ');
        }
    }
    let skip = collapsed.chars().count().saturating_sub(CONTEXT);
    let before: String = collapsed.chars().skip(skip).collect();
    let secret = String::from_utf8_lossy(secret);
    let shown: String = if secret.chars().count() >= 16 {
        secret
            .chars()
            .take(4)
            .take_while(|c| !c.is_whitespace())
            .collect()
    } else {
        String::new()
    };
    format!("{before}{shown}****")
}

/// The gitleaks release the default rules come from, such as `v8.30.1`.
pub fn gitleaks_version() -> &'static str {
    VERSION.trim()
}

//! Redacting with gitleaks' rules, when a copy of its config is at hand.
//!
//! [gitleaks](https://github.com/gitleaks/gitleaks) (MIT) keeps a couple of
//! hundred secret patterns in `config/gitleaks.toml`, each with an `id`, a
//! `regex`, optional `keywords` that must appear (case-insensitively) for
//! the regex to be tried, and an optional `secretGroup` naming the part of
//! a match that is the secret. This reads that format. A rule whose regex
//! does not compile with Rust's `regex` crate is skipped and counted; its
//! entropy and allowlists are ignored, so it redacts more, never less.

use regex::Regex;
use serde::Deserialize;
use std::path::Path;

#[derive(Debug)]
pub struct Gitleaks {
    rules: Vec<Rule>,
    /// Rules left out because their regex did not compile.
    pub skipped: usize,
}

#[derive(Debug)]
struct Rule {
    regex: Regex,
    keywords: Vec<String>,
    group: usize,
}

#[derive(Deserialize)]
struct Config {
    #[serde(default)]
    rules: Vec<RuleDecl>,
}

#[derive(Deserialize)]
struct RuleDecl {
    #[serde(default)]
    regex: Option<String>,
    #[serde(default)]
    keywords: Vec<String>,
    #[serde(default, rename = "secretGroup")]
    secret_group: usize,
}

impl Gitleaks {
    /// Reads a gitleaks config file.
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("reading {}: {e}", path.display()))?;
        Self::parse(&text).map_err(|e| format!("{}: {e}", path.display()))
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        let config: Config = toml::from_str(text).map_err(|e| e.to_string())?;
        let mut rules = Vec::new();
        let mut skipped = 0;
        for decl in config.rules {
            // Path-only rules (no `regex`) match file names, not text.
            let Some(pattern) = decl.regex else { continue };
            match Regex::new(&pattern) {
                Ok(regex) => rules.push(Rule {
                    regex,
                    keywords: decl.keywords.iter().map(|k| k.to_lowercase()).collect(),
                    group: decl.secret_group,
                }),
                Err(_) => skipped += 1,
            }
        }
        Ok(Self { rules, skipped })
    }

    pub fn rules(&self) -> usize {
        self.rules.len()
    }

    /// The text with each secret a rule finds replaced by `<redacted>`.
    pub fn redact(&self, text: &str) -> String {
        let mut out = text.to_string();
        let mut lower = out.to_lowercase();
        for rule in &self.rules {
            if !rule.keywords.is_empty() && !rule.keywords.iter().any(|k| lower.contains(k)) {
                continue;
            }
            let mut cuts: Vec<(usize, usize)> = Vec::new();
            for caps in rule.regex.captures_iter(&out) {
                let m = caps
                    .get(rule.group)
                    .or_else(|| caps.get(0))
                    .expect("a match has group 0");
                if !m.as_str().contains("<redacted>") && !m.is_empty() {
                    cuts.push((m.start(), m.end()));
                }
            }
            if cuts.is_empty() {
                continue;
            }
            for (start, end) in cuts.into_iter().rev() {
                out.replace_range(start..end, "<redacted>");
            }
            lower = out.to_lowercase();
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONFIG: &str = r#"
title = "a few gitleaks rules"

[[rules]]
id = "github-pat"
regex = '''ghp_[0-9a-zA-Z]{36}'''
keywords = ["ghp_"]

[[rules]]
id = "generic-api-key"
regex = '''(?i)(?:api_?key|secret)\s*[:=]\s*["']?([0-9a-z]{16,})'''
secretGroup = 1
keywords = ["api", "secret"]

[[rules]]
id = "lookaround"
regex = '''foo(?=bar)'''

[[rules]]
id = "pkcs12-file"
path = '''(?i)\.(?:p12|pfx)$'''
"#;

    #[test]
    fn redacts_what_its_rules_match() {
        let g = Gitleaks::parse(CONFIG).unwrap();
        assert_eq!((g.rules(), g.skipped), (2, 1));
        assert_eq!(
            g.redact(
                "token ghp_abcdefghijklmnopqrstuvwxyz0123456789 and API_KEY=abcd1234abcd1234zz"
            ),
            "token <redacted> and API_KEY=<redacted>"
        );
        assert_eq!(g.redact("nothing to see"), "nothing to see");
    }

    #[test]
    fn a_broken_file_is_an_error() {
        assert!(Gitleaks::parse("[[rules]\n").is_err());
    }
}

//! Secrets in text jouet keeps or shows: credentials are replaced before a
//! command reaches the ledger, and `jouet share` refuses text that still
//! looks like one.

use regex::Regex;
use std::sync::LazyLock;

/// How much of a match to keep: nothing, or its first group (`GH_TOKEN=`,
/// `Authorization: Bearer `), or its first group and a trailing `@`.
#[derive(Clone, Copy)]
enum Keep {
    Nothing,
    Prefix,
    PrefixAndAt,
}

/// Token formats with a recognisable prefix, and `NAME=value` assignments
/// whose name says it holds a secret.
static SECRETS: LazyLock<Vec<(Regex, Keep)>> = LazyLock::new(|| {
    [
        // GitHub, GitLab, Slack, OpenAI and Anthropic, AWS, Google, npm.
        (r"\b(gh[pousr]_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,})", Keep::Nothing),
        (r"\bglpat-[A-Za-z0-9_-]{20,}", Keep::Nothing),
        (r"\bxox[abposr]-[A-Za-z0-9-]{10,}", Keep::Nothing),
        (r"\bsk-(ant-)?[A-Za-z0-9_-]{20,}", Keep::Nothing),
        (r"\b(AKIA|ASIA)[A-Z0-9]{16}\b", Keep::Nothing),
        (r"\bAIza[A-Za-z0-9_-]{35}", Keep::Nothing),
        (r"\bnpm_[A-Za-z0-9]{36}", Keep::Nothing),
        (r"-----BEGIN [A-Z ]*PRIVATE KEY-----", Keep::Nothing),
        // An Authorization header's credentials.
        (r"(?i)\b(authorization:\s*(?:bearer|basic|token)\s+)[^\s'\x22]+", Keep::Prefix),
        // `GH_TOKEN=...`, `--password=...`, `api_key: ...`.
        (
            r"(?i)\b([A-Z0-9_]*(?:TOKEN|SECRET|PASSWORD|PASSWD|API_?KEY|ACCESS_?KEY|PRIVATE_?KEY)[A-Z0-9_]*\s*[=:]\s*)[^\s'\x22]+",
            Keep::Prefix,
        ),
        // Credentials in a URL: `https://user:pass@host`.
        (r"(?i)\b([a-z][a-z0-9+.-]*://[^\s/:@]+:)[^\s/@]+@", Keep::PrefixAndAt),
    ]
    .into_iter()
    .map(|(p, keep)| (Regex::new(p).expect("secret patterns compile"), keep))
    .collect()
});

pub(crate) const MASK: &str = "[redacted]";

/// The text with every secret replaced by `[redacted]`, keeping what shows
/// the shape of the command around it.
pub(crate) fn redact(text: &str) -> String {
    let mut out = text.to_string();
    for (re, keep) in SECRETS.iter() {
        out = re
            .replace_all(&out, |caps: &regex::Captures| {
                let prefix = caps.get(1).map_or("", |m| m.as_str());
                match keep {
                    Keep::Nothing => MASK.to_string(),
                    Keep::Prefix => format!("{prefix}{MASK}"),
                    Keep::PrefixAndAt => format!("{prefix}{MASK}@"),
                }
            })
            .into_owned();
    }
    out
}

/// The first secret left in the text, already redacted, if any.
pub(crate) fn find_secret(text: &str) -> Option<String> {
    SECRETS.iter().find_map(|(re, _)| {
        re.find_iter(text)
            .map(|m| m.as_str())
            .find(|m| !m.ends_with(MASK) && !m.contains(MASK))
            .map(redact)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_tokens_and_keeps_the_command_shape() {
        assert_eq!(
            redact("GH_TOKEN=ghp_abcdefghijklmnopqrstuvwxyz0123 gh pr list"),
            "GH_TOKEN=[redacted] gh pr list"
        );
        assert_eq!(
            redact("curl -H 'Authorization: Bearer abc.def' https://x"),
            "curl -H 'Authorization: Bearer [redacted]' https://x"
        );
        assert_eq!(
            redact("git clone https://me:hunter2@example.com/r.git"),
            "git clone https://me:[redacted]@example.com/r.git"
        );
        assert_eq!(
            redact("echo sk-ant-api03-abcdefghijklmnopqrstuv"),
            "echo [redacted]"
        );
        assert_eq!(redact("cargo test --all"), "cargo test --all");
    }

    #[test]
    fn finds_only_secrets_that_are_left() {
        assert!(find_secret("export AWS_KEY=AKIAABCDEFGHIJKLMNOP").is_some());
        assert_eq!(find_secret("GH_TOKEN=[redacted] gh pr list"), None);
        assert_eq!(find_secret("cargo build"), None);
        let found = find_secret("token: ghp_abcdefghijklmnopqrstuvwxyz0123").unwrap();
        assert!(!found.contains("ghp_abc"), "{found}");
    }
}

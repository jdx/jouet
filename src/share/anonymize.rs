//! Making a proposal safe to publish: paths, repository names, hostnames,
//! email addresses and the words the user names are replaced by
//! placeholders, so a rule learned in a private repository does not leak
//! where it was learned.

use regex::Regex;
use std::sync::LazyLock;

/// Hosts that say nothing about the user, kept as they are.
const PUBLIC_HOSTS: &[&str] = &["github.com", "jouet.jdx.dev", "luau.org", "www.luau.org"];

/// Top-level domains a hostname is recognised by. Luau field access such as
/// `string.sub` or `call.commands` never ends in one of these.
const TLDS: &str = "com|net|org|io|dev|app|ai|co|us|uk|de|fr|jp|cn|ru|eu|cloud|sh|internal|local|localdomain|corp|lan|intranet|home|test|example";

/// Each pattern and its replacement (`$1` keeps a pattern's first group).
static PATTERNS: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
    [
        // `git@host:owner/repo(.git)`, then `github.com/owner/repo` URLs.
        (r"\b[\w.-]+@[\w.-]+:[\w.-]+/[\w.-]+", "<repo>"),
        (
            r"(?i)\b(?:https?://)?(?:www\.)?(?:github|gitlab)\.com/[\w.-]+/[\w.-]+",
            "<repo>",
        ),
        // After repositories, whose `git@host:` looks like an address.
        (r"[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}", "<email>"),
        // Home directories, then any absolute path of two or more parts.
        (r"(?:~|\$HOME)(?:/[\w.@+-]+)+/?", "<path>"),
        (r"(^|[\s'\x22`(=:])/[\w.@+-]+(?:/[\w.@+-]*)+", "${1}<path>"),
        (r"\b[A-Za-z]:\\(?:[\w.@+ -]+\\?)+", "<path>"),
        // IPv4 addresses.
        (r"\b(?:\d{1,3}\.){3}\d{1,3}\b", "<host>"),
    ]
    .into_iter()
    .map(|(p, with)| (Regex::new(p).expect("anonymizing patterns compile"), with))
    .collect()
});

static HOSTS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"(?i)\b(?:[a-z0-9](?:[a-z0-9-]*[a-z0-9])?\.)+(?:{TLDS})\b"
    ))
    .expect("the host pattern compiles")
});

/// The text with private details replaced. `words` are more to replace,
/// such as internal tool names, the user's login and this machine's name;
/// each becomes `<redacted>`.
pub(crate) fn anonymize(text: &str, words: &[String]) -> String {
    let mut out = text.to_string();
    for (re, with) in PATTERNS.iter() {
        out = re.replace_all(&out, *with).into_owned();
    }
    out = HOSTS
        .replace_all(&out, |caps: &regex::Captures| {
            let host = &caps[0];
            if PUBLIC_HOSTS.iter().any(|h| h.eq_ignore_ascii_case(host)) {
                host.to_string()
            } else {
                "<host>".to_string()
            }
        })
        .into_owned();
    for word in words.iter().filter(|w| w.len() >= 2) {
        let re = Regex::new(&format!(r"\b{}\b", regex::escape(word))).expect("an escaped word");
        out = re.replace_all(&out, "<redacted>").into_owned();
    }
    out
}

/// Words of this machine to replace wherever they appear: the user's login,
/// the host's name, and the current repository's owner and name.
pub(crate) fn local_words() -> Vec<String> {
    let mut words = Vec::new();
    for var in ["USER", "USERNAME", "LOGNAME"] {
        if let Ok(v) = std::env::var(var) {
            words.push(v);
        }
    }
    let host = std::env::var("HOSTNAME")
        .ok()
        .or_else(|| std::fs::read_to_string("/etc/hostname").ok());
    if let Some(host) = host {
        let host = host.trim();
        words.push(host.to_string());
        if let Some((short, _)) = host.split_once('.') {
            words.push(short.to_string());
        }
    }
    if let Ok(cwd) = std::env::current_dir()
        && let Some(root) = cwd.ancestors().find(|d| d.join(".git").exists())
        && let Some(name) = root.file_name()
    {
        words.push(name.to_string_lossy().into_owned());
    }
    words.retain(|w| w.len() >= 3 && !matches!(w.as_str(), "root" | "user" | "runner" | "jouet"));
    words.sort();
    words.dedup();
    words
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_paths_repos_hosts_and_emails() {
        let text = "see /home/alice/work/acme-api/src/main.rs and ~/notes.md\n\
                    clone git@git.acme.internal:acme/billing.git or https://github.com/acme/billing\n\
                    mail ops@acme.com, host build-7.ci.acme.corp at 10.0.0.12\n\
                    docs at https://jouet.jdx.dev/rules and github.com\n";
        let out = anonymize(text, &["billing".into()]);
        assert_eq!(
            out,
            "see <path> and <path>\n\
             clone <repo> or <repo>\n\
             mail <email>, host <host> at <host>\n\
             docs at https://jouet.jdx.dev/rules and github.com\n"
        );
    }

    #[test]
    fn leaves_luau_code_alone() {
        let code = "local n = string.sub(arg, 1, 2)\nfor _, c in call.commands do\n\tif c.argv[1] == \"git\" then return jouet.deny(\"x\") end\nend\n-- a/b and warn/deny\n";
        assert_eq!(anonymize(code, &[]), code);
    }

    #[test]
    fn named_words_are_redacted_whole() {
        assert_eq!(
            anonymize("mytool sync; mytools", &["mytool".into()]),
            "<redacted> sync; mytools"
        );
    }
}

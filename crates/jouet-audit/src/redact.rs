//! Removing secrets and contents from anything a report shows.

use regex::Regex;
use std::sync::LazyLock;

/// Token-like strings, each replaced whole.
static TOKENS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        // Prefixed credentials: GitHub, GitLab, OpenAI/Anthropic, Slack, npm, PyPI, AWS, Google.
        r"\b(gh[pousr]_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,}|glpat-[A-Za-z0-9_-]{20,}",
        r"|sk-[A-Za-z0-9_-]{20,}|xox[abprs]-[A-Za-z0-9-]{10,}|npm_[A-Za-z0-9]{30,}",
        r"|pypi-[A-Za-z0-9_-]{30,}|AKIA[0-9A-Z]{16}|AIza[0-9A-Za-z_-]{30,}",
        // JSON web tokens.
        r"|eyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,})",
        // Long runs of key-like characters, checked by `key_like`.
        r"|([A-Za-z0-9+/_=-]{32,})",
    ))
    .expect("the token pattern compiles")
});

/// Values that follow a name saying they are secret: `TOKEN=...`,
/// `--password ...`, `Authorization: Bearer ...`, `https://user:pass@`.
/// A value is the whole shell word: quoted parts, escapes and all, so
/// `--password "a b"c` loses all of `"a b"c`.
static NAMED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"(?i)(\b[A-Z0-9_]*(?:TOKEN|SECRET|PASSWORD|PASSWD|API_?KEY|PRIVATE_KEY|CREDENTIALS?)[A-Z0-9_]*=)",
        r#"((?:"(?:[^"\\]|\\.)*"?|'[^']*'?|\\.|[^\s;&|"'\\])+)"#,
        r"|(--(?:token|password|passwd|secret|api-?key|auth-token|private-key|client-secret)(?:=|\s+))",
        r#"((?:"(?:[^"\\]|\\.)*"?|'[^']*'?|\\.|[^\s;&|"'\\])+)"#,
        r"|((?:authorization|x-api-key|private-token):\s*(?:bearer\s+|token\s+|basic\s+)?)([^\s'\x22]+)",
        r"|(://)([^/\s:@]+:[^/\s@]+)(@)",
    ))
    .expect("the named-secret pattern compiles")
});

/// Removes token-like strings: credentials with a known prefix, JSON web
/// tokens, values named as secrets, and any run of 32 or more key-like
/// characters (which also catches long hashes, deliberately).
pub fn redact(text: &str) -> String {
    let named = NAMED.replace_all(text, |c: &regex::Captures<'_>| {
        // A shape's placeholder is not a secret: `--token <s>` stays.
        let value = c.get(2).or_else(|| c.get(4)).map_or("", |m| m.as_str());
        if value.starts_with('<') && value.trim_end_matches('…').ends_with('>') {
            return c[0].to_string();
        }
        if let Some(name) = c.get(1) {
            format!("{}<redacted>", name.as_str())
        } else if let Some(flag) = c.get(3) {
            format!("{}<redacted>", flag.as_str())
        } else if let Some(header) = c.get(5) {
            format!("{}<redacted>", header.as_str())
        } else {
            "://<redacted>@".to_string()
        }
    });
    TOKENS
        .replace_all(&named, |c: &regex::Captures<'_>| match c.get(2) {
            Some(run) if !key_like(run.as_str()) => run.as_str().to_string(),
            _ => "<redacted>".to_string(),
        })
        .into_owned()
}

/// A long run reads as a key, not a name: mixed case with digits, base64
/// punctuation, or all hex. `my-very-long-worktree-name-590ea8` is a name.
fn key_like(run: &str) -> bool {
    let digit = run.bytes().any(|c| c.is_ascii_digit());
    let upper = run.bytes().any(|c| c.is_ascii_uppercase());
    let lower = run.bytes().any(|c| c.is_ascii_lowercase());
    let hex = run.bytes().all(|c| c.is_ascii_hexdigit());
    hex || run.contains(['+', '/', '=']) && digit || digit && upper && lower
}

/// A command as an example: heredoc bodies replaced by their line count,
/// the home directory written as `~`, secrets redacted, and anything past
/// `max_chars` cut with a marker saying how much was left out.
pub fn example(command: &str, home: Option<&str>, max_chars: usize) -> String {
    let mut out = String::new();
    let mut lines = command.lines();
    while let Some(line) = lines.next() {
        out.push_str(line);
        out.push('\n');
        let Some(delim) = heredoc_delimiter(line) else {
            continue;
        };
        let mut body = 0;
        let mut closed = false;
        for line in lines.by_ref() {
            if line.trim_start_matches('\t').trim_end() == delim {
                closed = true;
                out.push_str(&format!("<heredoc: {body} lines>\n{line}\n"));
                break;
            }
            body += 1;
        }
        if !closed {
            out.push_str(&format!("<heredoc: {body} lines>\n"));
        }
    }
    let mut out = out.trim_end().to_string();
    if let Some(home) = home.filter(|h| h.len() > 1) {
        out = out.replace(home, "~");
    }
    let out = redact(&out);
    let total = out.chars().count();
    if total <= max_chars {
        return out;
    }
    let mut cut: String = out.chars().take(max_chars).collect();
    cut.push_str(&format!(" … ({} more chars)", total - max_chars));
    cut
}

/// The delimiter of a heredoc a line starts, as in `cat <<'EOF'`.
fn heredoc_delimiter(line: &str) -> Option<String> {
    let at = line.find("<<")?;
    let rest = &line[at + 2..];
    if rest.starts_with('<') {
        return None;
    }
    let rest = rest.trim_start_matches('-').trim_start();
    let delim: String = rest
        .chars()
        .take_while(|c| !c.is_whitespace() && !";|&<>()".contains(*c))
        .filter(|c| !matches!(c, '\'' | '"' | '\\'))
        .collect();
    (!delim.is_empty()).then_some(delim)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_prefixed_and_named_secrets() {
        let ghp = format!("ghp_{}", "a1B2".repeat(9));
        assert_eq!(
            redact(&format!("gh auth login --with-token {ghp}")),
            "gh auth login --with-token <redacted>"
        );
        assert_eq!(
            redact("GITHUB_TOKEN=abc123 gh pr list"),
            "GITHUB_TOKEN=<redacted> gh pr list"
        );
        assert_eq!(redact("GH_TOKEN= gh pr list"), "GH_TOKEN= gh pr list");
        assert_eq!(redact("export API_KEY='x y'"), "export API_KEY=<redacted>");
        assert_eq!(
            redact("mytool --password hunter2"),
            "mytool --password <redacted>"
        );
        assert_eq!(
            redact(r#"mytool --password "correct horse" --next"#),
            "mytool --password <redacted> --next"
        );
        assert_eq!(
            redact("mytool --token   'a b c' x"),
            "mytool --token   <redacted> x"
        );
        assert_eq!(
            redact(r"mytool --api-key=a\ b\ c x"),
            "mytool --api-key=<redacted> x"
        );
        assert_eq!(redact(r#"API_KEY="a b"c rest"#), "API_KEY=<redacted> rest");
        assert_eq!(redact(r#"--secret "never closed"#), "--secret <redacted>");
        assert_eq!(
            redact("tool --token <s> TOKEN=<v>"),
            "tool --token <s> TOKEN=<v>"
        );
        assert_eq!(
            redact("curl -H 'Authorization: Bearer abc.def' https://u:p@host/x"),
            "curl -H 'Authorization: Bearer <redacted>' https://<redacted>@host/x"
        );
        assert_eq!(redact("sk-ant-api03-abcdefghijklmnopqrstuv"), "<redacted>");
        assert_eq!(
            redact("eyJhbGciOiJI.eyJzdWIiOiIx.SflKxwRJSMeKKF2"),
            "<redacted>"
        );
        assert_eq!(redact(&"f".repeat(40)), "<redacted>");
        assert_eq!(
            redact("cargo test -p jouet-audit"),
            "cargo test -p jouet-audit"
        );
        let name = "-home-coder-src-mise--claude-worktrees-agentic-cli-590ea8";
        assert_eq!(redact(name), name);
        assert_eq!(
            redact("Zm9vYmFyYmF6cXV4MTIzNDU2Nzg5MGFiY2RlZg"),
            "<redacted>"
        );
    }

    #[test]
    fn examples_drop_heredoc_bodies_and_the_home_directory() {
        let cmd = "cd /home/me/src/x && python3 - <<'EOF'\nsecret = 1\nprint(secret)\nEOF\necho ok";
        assert_eq!(
            example(cmd, Some("/home/me"), 200),
            "cd ~/src/x && python3 - <<'EOF'\n<heredoc: 2 lines>\nEOF\necho ok"
        );
        assert_eq!(
            example("cat <<EOF\nnever closed", None, 200),
            "cat <<EOF\n<heredoc: 1 lines>"
        );
        assert_eq!(example("echo abcdef", None, 6), "echo a … (5 more chars)");
    }
}

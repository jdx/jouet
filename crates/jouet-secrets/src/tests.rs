//! The fake secrets here are put together at run time, so this file holds
//! nothing a scanner (this one included) would take for a secret.

use super::*;
use std::process::Command;

/// A GitHub classic token's shape, with a made-up body.
fn github_pat() -> String {
    format!("ghp_{}", "Zq3XvB7mN2kL9pR4tY6wC1dF8gH0jK5sA2eU")
}

/// An AWS access key ID's shape that is not AWS's documented example.
fn aws_key() -> String {
    format!("AKIA{}", "Q7ZR4XKD2MWB6TLN")
}

fn scan_text(scanner: &Scanner, path: &str, text: &str) -> Report {
    let mut report = scanner.empty("files");
    scanner
        .text(&mut report, path, None, 1, text.as_bytes())
        .unwrap();
    report
}

#[test]
fn every_default_rule_compiles() {
    let rules = Rules::default_rules();
    assert!(rules.len() > 200, "{} rules", rules.len());
    assert_eq!(rules.compile_all(), Vec::<String>::new());
}

#[test]
fn finds_a_token_and_never_shows_it() {
    let scanner = Scanner::for_repo(Path::new("/nonexistent")).unwrap();
    let pat = github_pat();
    let text = format!("fn main() {{}}\nlet token = \"{pat}\";\n");
    let report = scan_text(&scanner, "src/main.rs", &text);
    let rules: Vec<&str> = report.findings.iter().map(|f| f.rule.as_str()).collect();
    // generic-api-key matches it too, and is dropped for the specific rule.
    assert_eq!(rules, ["github-pat"]);
    let f = report
        .findings
        .iter()
        .find(|f| f.rule == "github-pat")
        .unwrap();
    assert_eq!(f.line, 2);
    assert_eq!(f.redacted, "ghp_****");
    assert_eq!(f.fingerprint, "src/main.rs:github-pat:2");
    let json = serde_json::to_string(&report).unwrap();
    assert!(!json.contains(&pat[4..]), "{json}");
}

#[test]
fn allowlists_stopwords_and_inline_allows_pass_findings_over() {
    let scanner = Scanner::for_repo(Path::new("/nonexistent")).unwrap();
    // AWS's documented example key is on the rule's allowlist.
    let example = format!("aws_access_key_id = {}\n", "AKIAIOSFODNN7EXAMPLE");
    assert!(scan_text(&scanner, "a.env", &example).findings.is_empty());
    let key = aws_key();
    let real = format!("aws_access_key_id = {key}\n");
    let found = scan_text(&scanner, "a.env", &real);
    assert!(
        found.findings.iter().any(|f| f.rule == "aws-access-token"),
        "{found:?}"
    );
    let inline = format!("aws_access_key_id = {key} # gitleaks:allow\n");
    assert!(scan_text(&scanner, "a.env", &inline).findings.is_empty());
    // The global path allowlist: lockfiles are not scanned.
    let lock = "pnpm-lock.yaml";
    assert!(scanner.rules.path_allowed(lock).unwrap());
}

#[test]
fn generic_rules_want_entropy_and_a_digit() {
    let scanner = Scanner::for_repo(Path::new("/nonexistent")).unwrap();
    let low = "api_key = \"aaaaaaaaaaaaaaaaaaaa\"\n";
    assert!(scan_text(&scanner, "c.py", low).findings.is_empty());
    let letters = "api_key = \"qwhzkxmvbnrtplsjdfgy\"\n";
    assert!(scan_text(&scanner, "c.py", letters).findings.is_empty());
    let secret = format!("api_key = \"{}\"\n", "q8Wz3Kx7Mv2Bn9Rt4Pl6");
    let found = scan_text(&scanner, "c.py", &secret);
    assert_eq!(
        found
            .findings
            .iter()
            .map(|f| (f.rule.as_str(), f.redacted.as_str()))
            .collect::<Vec<_>>(),
        [("generic-api-key", "api_key = \"q8Wz****")]
    );
}

#[test]
fn a_repository_config_extends_or_replaces_the_defaults() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join(".gitleaks.toml"),
        "[extend]\nuseDefault = true\ndisabledRules = [\"github-pat\"]\n\n\
         [[rules]]\nid = \"acme-token\"\nregex = '''acme_[a-z0-9]{12}'''\nkeywords = [\"acme_\"]\n\n\
         [[allowlists]]\npaths = ['''^fixtures/''']\n",
    )
    .unwrap();
    let scanner = Scanner::for_repo(dir.path()).unwrap();
    assert!(scanner.config.contains(".gitleaks.toml"));
    let text = format!("a = acme_{}\nb = {}\n", "x1y2z3w4v5u6", github_pat());
    let found = scan_text(&scanner, "src/x.rs", &text);
    let rules: Vec<&str> = found.findings.iter().map(|f| f.rule.as_str()).collect();
    assert!(rules.contains(&"acme-token"), "{rules:?}");
    assert!(!rules.contains(&"github-pat"), "{rules:?}");
    assert!(scanner.rules.path_allowed("fixtures/a.txt").unwrap());

    std::fs::write(
        dir.path().join(".gitleaks.toml"),
        "[[rules]]\nid = \"only\"\nregex = '''only_[0-9]{4}'''\n",
    )
    .unwrap();
    assert_eq!(Scanner::for_repo(dir.path()).unwrap().rules.len(), 1);
    std::fs::write(dir.path().join(".gitleaks.toml"), "[[rules]\n").unwrap();
    let err = Scanner::for_repo(dir.path()).err().unwrap();
    assert_eq!(err.exit, Exit::Precondition);
}

#[test]
fn gitleaksignore_and_allowances_pass_over_fingerprints() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join(".gitleaksignore"),
        "# known\nsrc/main.rs:github-pat:2\n",
    )
    .unwrap();
    let mut scanner = Scanner::for_repo(dir.path()).unwrap();
    let text = format!("x\nt = \"{}\"\nk = {}\n", github_pat(), aws_key());
    let found = scan_text(&scanner, "src/main.rs", &text);
    assert_eq!(found.ignored, 1);
    assert!(found.findings.iter().all(|f| f.rule != "github-pat"));
    scanner.allow("src/main.rs:aws-access-token:3".into());
    let found = scan_text(&scanner, "src/main.rs", &text);
    assert_eq!(found.ignored, 2);
    assert!(found.findings.is_empty(), "{:?}", found.findings);
}

fn git(dir: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .current_dir(dir)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.com")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.com")
        .status()
        .unwrap()
        .success();
    assert!(ok, "git {args:?}");
}

#[test]
fn staged_lines_and_commits_are_scanned_with_their_line_numbers() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    git(root, &["init", "-q"]);
    std::fs::write(root.join("a.txt"), "one\ntwo\n").unwrap();
    git(root, &["add", "a.txt"]);
    git(root, &["commit", "-q", "-m", "first"]);
    std::fs::write(
        root.join("a.txt"),
        format!("one\ntwo\ntoken {}\n", github_pat()),
    )
    .unwrap();
    git(root, &["add", "a.txt"]);
    let scanner = Scanner::for_repo(root).unwrap();
    let staged = Source::Staged {
        paths: Vec::new(),
        index: None,
    };
    let report = scanner.scan(root, root, &staged).unwrap();
    assert_eq!(report.files_scanned, 1);
    let lines: Vec<(&str, u64)> = report
        .findings
        .iter()
        .map(|f| (f.rule.as_str(), f.line))
        .collect();
    assert_eq!(lines, [("github-pat", 3)]);

    git(root, &["commit", "-q", "-m", "add a token"]);
    std::fs::write(root.join("a.txt"), "one\ntwo\n").unwrap();
    git(root, &["commit", "-q", "-am", "remove it"]);
    let commits = Source::Commits {
        revs: vec!["HEAD".into()],
        paths: Vec::new(),
    };
    let report = scanner.scan(root, root, &commits).unwrap();
    assert_eq!(report.commits_scanned, Some(3));
    assert_eq!(report.findings.len(), 1, "{:?}", report.findings);
    let f = &report.findings[0];
    assert_eq!(f.line, 3);
    assert!(f.fingerprint.ends_with(":a.txt:github-pat:3"), "{f:?}");
    assert_eq!(f.commit.as_deref().map(str::len), Some(40));

    // The working tree no longer has it.
    let files = Source::Files(Vec::new());
    assert!(
        scanner
            .scan(root, root, &files)
            .unwrap()
            .findings
            .is_empty()
    );
}

#[test]
fn files_are_walked_as_git_sees_them() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    git(root, &["init", "-q"]);
    std::fs::write(root.join(".gitignore"), "ignored.txt\n").unwrap();
    std::fs::write(root.join("ignored.txt"), github_pat()).unwrap();
    std::fs::create_dir(root.join("sub")).unwrap();
    std::fs::write(root.join("sub/.hidden"), github_pat()).unwrap();
    std::fs::write(root.join("bin.dat"), b"\0\x01ghp_").unwrap();
    std::fs::write(root.join("key.p12"), b"\0").unwrap();
    let scanner = Scanner::for_repo(root).unwrap();
    let report = scanner
        .scan(root, root, &Source::Files(Vec::new()))
        .unwrap();
    let found: Vec<(&str, &str)> = report
        .findings
        .iter()
        .map(|f| (f.path.as_str(), f.rule.as_str()))
        .collect();
    assert_eq!(
        found,
        [("key.p12", "pkcs12-file"), ("sub/.hidden", "github-pat")]
    );
    assert_eq!(report.binary_files, 2);
    // Named outright, an ignored file is read; from a subdirectory, paths
    // stay relative to the root.
    let named = Source::Files(vec!["../ignored.txt".into()]);
    let report = scanner.scan(root, &root.join("sub"), &named).unwrap();
    assert_eq!(report.findings[0].path, "ignored.txt");
    let missing = Source::Files(vec!["nope".into()]);
    assert_eq!(
        scanner.scan(root, root, &missing).err().unwrap().exit,
        Exit::NotFound
    );
}

#[test]
fn redaction_keeps_little() {
    assert_eq!(rules::redact(b"key: ", b"short"), "key: ****");
    assert_eq!(
        rules::redact(b"x\n  export   TOKEN=", b"abcdefghijklmnopqrstuvwxyz"),
        "export TOKEN=abcd****"
    );
    let long = "a".repeat(80);
    assert_eq!(rules::redact(long.as_bytes(), b"").len(), 44);
}

#[test]
fn go_regexes_are_read_as_go_reads_them() {
    assert_eq!(rules::from_go(r"^\$(?:\d+|{\d+})$"), r"^\$(?:\d+|\{\d+\})$");
    assert_eq!(rules::from_go(r"a{2,5}b{3}c{1,}"), r"a{2,5}b{3}c{1,}");
    assert_eq!(rules::from_go(r"\{{[^}]+}}"), r"\{\{[^}]+\}\}");
    assert_eq!(rules::from_go(r"[[:alnum:]_[]"), r"[[:alnum:]_\[]");
    assert_eq!(rules::from_go(r"[]a&&b~~]"), r"[\]a\&\&b\~\~]");
}

use usage_rs::{Args, Subcommands};

/// Find secrets in files, in what is staged, or in commits, with gitleaks' rules
#[derive(Args)]
pub(crate) struct SecretsArgs {
    #[usage(subcommand)]
    pub(crate) cmd: SecretsCmd,
}

#[derive(Subcommands)]
pub(crate) enum SecretsCmd {
    /// Find API keys, tokens and private keys, with gitleaks' rules
    Scan(SecretsScanArgs),
}

/// Find API keys, tokens and private keys, with gitleaks' rules
///
/// Uses gitleaks' detection rules (github.com/gitleaks/gitleaks, MIT), built
/// in: each rule's keywords, regex, entropy and allowlists, and gitleaks'
/// own allowlists, which leave out lockfiles, images and vendored code. A
/// repository's .gitleaks.toml adds rules and allowlists ([extend]
/// useDefault = true, or allowlists alone) or replaces the rules. A line
/// with `gitleaks:allow` on it, and a finding whose fingerprint
/// (PATH:RULE:LINE) is in .gitleaksignore, are passed over.
///
/// Without --staged or --diff, scans the files and directories given (the
/// current directory by default) as they are on disk; directories are
/// walked as git sees them, so ignored files are left out unless named.
/// --staged scans only the lines the index adds to HEAD, which is what `git
/// commit` would record. --diff REF scans the lines each commit in REF..HEAD
/// adds, commit by commit, so a secret added and removed again is still
/// found; REF may also be a range, A..B. With either, the paths limit the
/// diff.
///
/// Each finding names its rule, file and line, with the match redacted to at
/// most the secret's first four characters; the secret itself is never
/// printed. Exits 1 when anything is found, 3 for a path or REF that does not
/// exist, and 4 outside a git repository with --staged or --diff, or for a
/// .gitleaks.toml that does not parse.
///
/// Examples:
///   jouet secrets scan
///   jouet secrets scan --staged
///   jouet secrets scan --diff origin/main
///   jouet secrets scan config/ .env.example
#[derive(Args)]
pub(crate) struct SecretsScanArgs {
    /// Files and directories to scan; with --staged or --diff, the paths
    /// the diff is limited to
    pub(crate) paths: Vec<String>,
    /// Scan the lines the index adds to HEAD: what `git commit` would record
    #[usage(long)]
    pub(crate) staged: bool,
    /// Scan the lines each commit in REF..HEAD adds, or in a range A..B
    #[usage(long, value_name = "REF")]
    pub(crate) diff: Option<String>,
}

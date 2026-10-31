use usage_rs::Args;

/// Commit only the named paths, with trailers
///
/// Commits exactly the paths given, as they are in the working tree (`git
/// commit --only`): anything else already staged stays staged, and untracked
/// files are added only when a named path covers them. Refuses -A, `.`, the
/// repository root and a path with no changes. Paths are literal, never
/// globs. Appends the configured trailers; inside an agent the default is a
/// co-author line (Claude Code) and the AI disclosure, with the tool and
/// version from AI_AGENT. Warns about a subject that is not a conventional
/// commit when the repository checks them. Hooks always run; when one fails,
/// the end of its output is shown and the whole of it kept in a log.
///
/// Before committing, the lines the commit adds are scanned with gitleaks'
/// rules (as `jouet secrets scan --staged` does), and in a project that uses
/// fnox, `fnox scan --values` checks them for its secrets' values. A secret
/// found refuses the commit (exit 1), naming the rule or secret and the
/// file and line, never the value. A false positive is let through with
/// --allow-secret RULE:PATH:LINE, `gitleaks:allow` on its line, or its
/// fingerprint in .gitleaksignore.
#[derive(Args)]
pub(crate) struct CommitArgs {
    /// The files and directories to commit
    pub(crate) paths: Vec<String>,
    /// The message. Repeatable: each is a paragraph, as with git
    #[usage(short = 'm', long, var, value_name = "MSG")]
    pub(crate) message: Vec<String>,
    /// Read the message from FILE (`-` is stdin)
    #[usage(short = 'F', long, value_name = "FILE")]
    pub(crate) file: Option<String>,
    /// Append a trailer, `Key: value`. Repeatable
    #[usage(long, var, value_name = "TRAILER")]
    pub(crate) trailer: Vec<String>,
    /// Leave out the configured and default trailers
    #[usage(long)]
    pub(crate) no_trailers: bool,
    /// Show what would be committed, committing nothing
    #[usage(long)]
    pub(crate) dry_run: bool,
    /// Commit this finding of the secret scan anyway, a false positive:
    /// RULE:PATH:LINE as the refusal lists it. Repeatable
    #[usage(long, var, value_name = "RULE:PATH:LINE")]
    pub(crate) allow_secret: Vec<String>,
    /// Refused: name the paths to commit instead
    #[usage(short = 'A', long, hide)]
    pub(crate) all: bool,
    /// Refused: jouet commit always runs hooks
    #[usage(long, hide)]
    pub(crate) no_verify: bool,
}

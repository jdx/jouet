use usage_rs::{Args, Subcommands};

/// Report the errors and warnings in files you changed, through hk
///
/// With an hk config (hk.pkl), runs `hk run check --safe --format json` on
/// exactly the files given, or with --changed (the default) those that
/// differ from HEAD or are untracked, and prints hk's normalised findings:
/// file, line, column, severity, rule, message and the step that found it.
/// A step that sets no `diagnostic_format` is reported with its status and
/// the end of its output. Without an hk config, falls back to cargo clippy,
/// ruff or shellcheck in their JSON modes, or says diagnostics are
/// unavailable. Exits 0 when clean, 1 with findings, 4 when hk refused to run
/// (`--safe` rejects commands whose effect is unknown or destructive) or
/// nothing can check the files.
#[derive(Args)]
pub(crate) struct DiagArgs {
    #[usage(subcommand)]
    pub(crate) cmd: DiagCmd,
}

#[derive(Subcommands)]
pub(crate) enum DiagCmd {
    /// Check files and report every finding
    Check(DiagCheckArgs),
    /// Report only the findings an edit introduced
    Delta(DiagDeltaArgs),
    /// Format files with the project's fixers (`hk run fix --safe`)
    Fix(DiagFixArgs),
}

/// Check files and report every finding
///
/// Each check also remembers, per file, what it found in that version of
/// the file, which is what `jouet diag delta` compares with.
#[derive(Args)]
pub(crate) struct DiagCheckArgs {
    /// Files to check (default: --changed)
    pub(crate) files: Vec<String>,
    /// The files that differ from HEAD or are untracked
    #[usage(long)]
    pub(crate) changed: bool,
    /// Show at most N findings (default 50); the rest are counted
    #[usage(long, value_name = "N")]
    pub(crate) max: Option<usize>,
}

/// Report only the findings an edit introduced
///
/// Checks the files, then compares each with the last check of an earlier
/// version of it: a finding is the same one when its file, rule and step
/// match and its message matches or it is within three lines, so findings
/// that only moved are not new. A file unchanged since HEAD has nothing new.
/// A changed file no check has seen before has no baseline; its findings are
/// listed apart and not counted. Exits 1 when the edit introduced a finding
/// or made a step without diagnostics fail.
#[derive(Args)]
pub(crate) struct DiagDeltaArgs {
    /// Files to check (default: --changed)
    pub(crate) files: Vec<String>,
    /// The files that differ from HEAD or are untracked
    #[usage(long)]
    pub(crate) changed: bool,
    /// Show at most N findings (default 50); the rest are counted
    #[usage(long, value_name = "N")]
    pub(crate) max: Option<usize>,
}

/// Format files with the project's fixers (`hk run fix --safe`)
///
/// Runs hk's fix steps on exactly these files, refusing (exit 4) a config
/// with a fixer whose effect is unknown or destructive, and reports which
/// files changed. Needs an hk config.
#[derive(Args)]
pub(crate) struct DiagFixArgs {
    /// Files to format (default: --changed)
    pub(crate) files: Vec<String>,
    /// The files that differ from HEAD or are untracked
    #[usage(long)]
    pub(crate) changed: bool,
    /// Only this hk step. Repeatable
    #[usage(long, var, value_name = "STEP")]
    pub(crate) step: Vec<String>,
    /// Say what would run, changing nothing
    #[usage(long)]
    pub(crate) dry_run: bool,
}

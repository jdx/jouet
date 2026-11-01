use usage_rs::{Args, Subcommands};

/// Make exact, all-or-nothing edits across files
///
/// Reads a spec from --spec or stdin: search/replace blocks, a Codex-style
/// patch, or JSON. Every edit is checked before any file is written, so a
/// failed edit leaves every file as it was.
///
/// A SEARCH block holds whole lines, indentation included: one that holds
/// part of a line matches nothing, and the failure says which line the text
/// is inside. `jouet edit replace` matches text anywhere, inside a line too.
///
/// A blocks section headed `=== PATH#SYMBOL` replaces a whole definition,
/// found with tree-sitter, so a formatter reflowing its text cannot break
/// the match. SYMBOL is a name, a kind and a name (`fn download`), or a
/// qualified name (`Vfox::download`), as `jouet read FILE --outline` lists
/// them; it must match exactly one definition. The new text runs to the
/// next `===` line, at any indentation: it is indented to where the
/// definition sits. Comments and attributes above the definition stay. An
/// edit that would add syntax errors is refused. When an edit changes the
/// definition's first line, its callers in other files are listed from the
/// code graph (`jouet graph`), or the change is noted when there is none.
///
///   === src/backend/vfox.rs#Vfox::download
///   pub fn download(&self, url: &str, retries: u32) -> Result<()> {
///       ...
///   }
///
/// `--pattern PAT --rewrite REPL --lang LANG [PATH...]` rewrites every
/// match of a structural pattern (ast-grep's) under the paths, instead of
/// reading a spec. The rewrites go through the same all-or-nothing writes,
/// --dry-run, --diff, --check and --revert-on-fail, a rewrite that adds
/// syntax errors is refused, and a pattern that matches nothing writes
/// nothing (exit 4). A pattern is code in LANG:
///
/// - Quote it in single quotes. In double quotes the shell expands `$X`,
///   and `"$X.unwrap()"` becomes `.unwrap()`, which matches nothing.
/// - `$X` (uppercase) matches exactly one node; `$$$X` matches any number,
///   such as all of a call's arguments. `foo($A)` matches only one-argument
///   calls, `foo($$$ARGS)` every call. `$_` matches without capturing.
/// - It must be whole code: `client.get($URL)`, not `get($URL`. A Rust
///   call pattern matches the call, not a `?` or `;` after it.
/// - A name used twice must match the same text twice: `$A == $A`.
/// - REPL is text with the pattern's `$X` filled in; the rest is copied as
///   written, and indentation follows the match.
///
/// `--callers-of PATH#SYMBOL` rewrites only the calls the code graph
/// attributes to that definition, so `load($$$A)` leaves other functions
/// named `load` alone; write the pattern for the whole call (`$F($P)`,
/// `$R.load($$$A)`). `--impact PATH#SYMBOL` rewrites only inside it and the
/// code depending on it. Matches the graph cannot attribute (dynamic
/// dispatch, macros, files it has not read) are listed as unresolved and
/// left alone, and so are its calls the pattern missed. The report lists
/// each rewrite and the test files that depend on them; --dry-run first.
///
/// Examples:
///   jouet edit --pattern '$C.get($URL).send()?' --rewrite '$C.get($URL).timeout(TIMEOUT).send()?' --lang rust src/
///   jouet edit --pattern 'print($$$ARGS)' --rewrite 'log.info($$$ARGS)' --lang python app/ --check 'python -m pytest -q' --revert-on-fail
///   jouet edit --dry-run --pattern '$F($P)' --rewrite '$F($P, None)' --lang rust --callers-of src/config.rs#load
///
/// Try a pattern with `jouet search --structural --lang LANG 'PAT'` first.
#[derive(Args)]
pub(crate) struct EditArgs {
    /// Read the spec from FILE instead of stdin (`-` is stdin)
    #[usage(long, value_name = "FILE")]
    pub(crate) spec: Option<String>,
    /// Spec format: blocks, patch or json. Detected from the first line
    #[usage(long, value_name = "FORMAT")]
    pub(crate) format: Option<String>,
    /// Report what would change without writing anything
    #[usage(long, global)]
    pub(crate) dry_run: bool,
    /// Include a unified diff of each changed file
    #[usage(long, global)]
    pub(crate) diff: bool,
    /// Run CMD through the shell after writing
    #[usage(long, global, value_name = "CMD")]
    pub(crate) check: Option<String>,
    /// Fail if the edit introduced diagnostics: the edited files are checked
    /// through hk (`jouet diag`) before and after writing, and only new
    /// findings count
    #[usage(long, global)]
    pub(crate) diag: bool,
    /// Restore every edited file if --check or --diag fails
    #[usage(long, global)]
    pub(crate) revert_on_fail: bool,
    /// Warn when the edit changes lines an earlier agent session wrote, by
    /// `entire why`: the session, checkpoint and prompt behind them, so
    /// `--dry-run --why` shows it before anything is written. It warns and
    /// does not refuse. Needs Entire enabled in the repository; off by
    /// default because `entire why` can take a minute where the history is
    /// long. Looks at the first 10 files
    #[usage(long, global)]
    pub(crate) why: bool,
    /// Rewrite code matching this ast-grep pattern, instead of reading a spec
    #[usage(long, value_name = "PAT")]
    pub(crate) pattern: Option<String>,
    /// What --pattern's matches become; its `$X` are the pattern's
    #[usage(long, value_name = "REPL")]
    pub(crate) rewrite: Option<String>,
    /// The language of --pattern: rust, python, javascript, typescript, tsx,
    /// go, ruby, java, c or cpp
    #[usage(long, value_name = "LANG")]
    pub(crate) lang: Option<String>,
    /// With --pattern, rewrite only the calls the code graph attributes to
    /// this symbol, given as PATH#NAME (or #ID from `jouet graph symbols`)
    #[usage(long, value_name = "PATH#SYMBOL")]
    pub(crate) callers_of: Option<String>,
    /// With --pattern, rewrite only inside this symbol and the code the
    /// graph says depends on it, three hops back
    #[usage(long, value_name = "PATH#SYMBOL")]
    pub(crate) impact: Option<String>,
    /// With --pattern, the files and directories to rewrite (default .)
    pub(crate) paths: Vec<String>,
    /// Refuse unless PATH's contents have this SHA-256 hash. Repeatable
    #[usage(long, global, value_name = "PATH=SHA256")]
    pub(crate) expect_hash: Vec<String>,
    #[usage(subcommand)]
    pub(crate) cmd: Option<EditCmd>,
}

#[derive(Subcommands)]
pub(crate) enum EditCmd {
    /// Replace text in files, given on the command line
    Replace(ReplaceArgs),
    /// Print the SHA-256 hash of files, for --expect-hash
    Hash(HashArgs),
}

/// Replace text in files, given on the command line
///
/// Matches anywhere in a file, like `str::replace`, and requires exactly one
/// match in each file unless --count or --all says otherwise. Repeat --old
/// and --new for several replacements: the Nth --new replaces the Nth
/// --old, in order. Every replacement applies to every file, and every file
/// is checked before any is written, so a rename across files is one call
/// that lands whole or not at all.
///
/// Examples:
///   jouet edit replace src/lib.rs --old 'let x = 1;' --new 'let x = 2;'
///   jouet edit replace a.py b.py --old retry_limit --new max_attempts --all
///   jouet edit replace app.py --old 'a()' --new 'b()' --old 'c()' --new 'd()'
#[derive(Args)]
pub(crate) struct ReplaceArgs {
    /// The files to edit
    pub(crate) paths: Vec<String>,
    /// The text to find. Repeatable. It may start with `-`, such as a
    /// Markdown list item
    #[usage(long, var, value_name = "TEXT", allow_hyphen_values)]
    pub(crate) old: Vec<String>,
    /// The text to put in its place, one for each --old. It may start with
    /// `-` too
    #[usage(long, var, value_name = "TEXT", allow_hyphen_values)]
    pub(crate) new: Vec<String>,
    /// How many matches each --old must have in each file
    #[usage(long, value_name = "N")]
    pub(crate) count: Option<String>,
    /// Replace every match (at least one in each file)
    #[usage(long)]
    pub(crate) all: bool,
}

/// Print the SHA-256 hash of files, for --expect-hash
#[derive(Args)]
pub(crate) struct HashArgs {
    /// Files to hash
    pub(crate) paths: Vec<String>,
}

use usage_rs::Args;

/// Search files like rg, within a budget, grouped by symbol
///
/// Embeds ripgrep's walker and searcher, so it respects .gitignore, skips
/// hidden and binary files, and is about as fast as rg. Patterns are rg's
/// regular expressions (-F for a literal). Beyond rg:
///
/// - Matches are grouped by file, then by the function, method, class or
///   module they are in (for Rust, Python, JavaScript, TypeScript, Go,
///   Ruby, Java, C and C++).
/// - Output stops at a byte budget, in total and per file, and says what it
///   left out: `... 4 more in src/x.rs`, and a closing line naming the files
///   it never reached. Nothing is cut silently; --budget 0 removes a limit.
/// - Inside an agent session (JOUET_SESSION_ID, CLAUDE_CODE_SESSION_ID or
///   CODEX_THREAD_ID), a match this session was already shown, in a file
///   that has not changed since, collapses to one line (`src/x.rs: 3 already
///   shown, unchanged`). --again shows them anyway.
///
/// With --structural, PATTERN is code in --lang, matched by ast-grep: `$X`
/// is one node, `$$$X` any number, and only files in that language are
/// searched. Quote it in single quotes, or the shell expands `$X`
/// (`jouet agent-help edit` has more on patterns).
///
/// Exits 0 with matches, 1 with none, 2 for a bad pattern or glob, and 3
/// for a path that does not exist.
///
/// Examples:
///   jouet search 'fn download' src/
///   jouet search -F 'client.get(' -g '*.rs'
///   jouet search -i timeout --budget 2000 --file-budget 400
///   jouet search -l TODO
///   jouet search --structural --lang rust '$X.unwrap()'
///   jouet search --structural --lang go 'if err != nil { return $$$ }'
///   jouet search --structural --lang typescript 'useEffect($FN, [])'
///   jouet search --structural --lang rust '$F($$$A)' --callers-of src/config.rs#load

#[derive(Args)]
pub(crate) struct SearchArgs {
    /// The regular expression to search for (a literal with -F)
    pub(crate) pattern: String,
    /// Files and directories to search (default .)
    pub(crate) paths: Vec<String>,
    /// Treat the pattern as a literal string
    #[usage(short = 'F', long)]
    pub(crate) fixed_strings: bool,
    /// Match case-insensitively
    #[usage(short = 'i', long)]
    pub(crate) ignore_case: bool,
    /// Match whole words only
    #[usage(short = 'w', long)]
    pub(crate) word_regexp: bool,
    /// Include or exclude files, as rg's -g: `*.rs`, `!*_test.go`. Repeatable
    #[usage(short = 'g', long, var, value_name = "GLOB")]
    pub(crate) glob: Vec<String>,
    /// Search hidden files and directories too
    #[usage(long)]
    pub(crate) hidden: bool,
    /// List the matching files with their match counts, not the lines
    #[usage(short = 'l', long)]
    pub(crate) files_with_matches: bool,
    /// The most bytes to print in total (default 6000; 0 is no limit)
    #[usage(long, value_name = "BYTES")]
    pub(crate) budget: Option<usize>,
    /// The most bytes to print from one file (default 1500; 0 is no limit)
    #[usage(long, value_name = "BYTES")]
    pub(crate) file_budget: Option<usize>,
    /// Show matches this session was already shown, too
    #[usage(long)]
    pub(crate) again: bool,
    /// Match PATTERN as code, with ast-grep: `$X` is one node, `$$$X` any
    /// number. Needs --lang
    #[usage(long)]
    pub(crate) structural: bool,
    /// The language of a --structural pattern: rust, python, javascript,
    /// typescript, tsx, go, ruby, java, c or cpp
    #[usage(long, value_name = "LANG")]
    pub(crate) lang: Option<String>,
    /// With --structural, only the calls the code graph attributes to this
    /// symbol, given as PATH#NAME (or #ID from `jouet graph symbols`)
    #[usage(long, value_name = "PATH#SYMBOL")]
    pub(crate) callers_of: Option<String>,
    /// With --structural, only matches inside this symbol and the code the
    /// graph says depends on it, three hops back
    #[usage(long, value_name = "PATH#SYMBOL")]
    pub(crate) impact: Option<String>,
}

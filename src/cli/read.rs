use usage_rs::Args;

/// Read a file by its outline, a symbol or a range, or the repository's map
///
/// Reading a large file whole to find one thing fills the context with code
/// that is not needed. Read it in steps instead:
///
/// 1. `--outline`: every definition with its line range, as `kind name
///    (start-end)`, the name qualified by its scope (`Vfox::download`).
/// 2. `--symbol NAME`: one definition's code. NAME is a name, a kind and a
///    name (`fn download`, `class Cart`), or a qualified name
///    (`Vfox::download`, `Shop.Cart`); every definition it matches is printed.
/// 3. `--range A:B`: lines A to B (`A:` to the end, `:B` from the start).
///
/// With none of them, a file of up to 300 lines is printed whole, and a
/// longer one as its outline. `--outline` prints a file of up to 100 lines
/// whole too (with its outline in --json), since reading it after its
/// outline would cost another call. Code is printed with line numbers, at most
/// --max-lines lines (default 400; 0 is no limit) with a marker for the
/// rest. Inside an agent session, lines this session was already shown, from
/// a file unchanged since, collapse to one line; --again prints them.
/// Definitions come from tree-sitter, for Rust, Python, JavaScript,
/// TypeScript, Go, Ruby, Java, C and C++.
///
/// `--map` prints, for the files that matter most, the line of each of
/// their most important definitions, sized to a token budget, so an agent
/// can find its way around a repository without reading whole files. It is
/// Aider's repo map on the code graph (`jouet graph`): files are ranked by
/// personalised PageRank over which file uses which definition, as the
/// graph resolved them, and as many top-ranked definitions are shown as fit
/// the budget. --focus names a file you are working on, or an identifier you
/// care about: what it uses ranks higher, and a focused file's own
/// definitions are left out, as you already have it open. The budget
/// defaults to 1024 tokens with --focus and 8192 without. With a PATH, only
/// files under that directory are listed.
///
/// Exits 3 for a file or symbol that does not exist, and 2 for an outline or
/// symbol of a file whose language jouet cannot read.
///
/// Examples:
///   jouet read src/backend/vfox.rs --outline
///   jouet read src/backend/vfox.rs --symbol 'Vfox::download'
///   jouet read src/backend/vfox.rs --range 120:160
///   jouet read --map --focus src/backend/vfox.rs --focus download
#[derive(Args)]
pub(crate) struct ReadArgs {
    /// The file to read, or with --map the directory to map (default .)
    pub(crate) path: Option<String>,
    /// List the file's definitions and their line ranges (a file of up to
    /// 100 lines is printed whole)
    #[usage(long)]
    pub(crate) outline: bool,
    /// Print the definitions this names: NAME, KIND NAME or Scope::NAME
    #[usage(long, value_name = "NAME")]
    pub(crate) symbol: Option<String>,
    /// Print lines A to B: `A:B`, `A:` or `:B`
    #[usage(long, value_name = "A:B")]
    pub(crate) range: Option<String>,
    /// The most lines of code to print (default 400; 0 is no limit)
    #[usage(long, value_name = "N")]
    pub(crate) max_lines: Option<usize>,
    /// Print lines this session was already shown, too
    #[usage(long)]
    pub(crate) again: bool,
    /// With --symbol or --range, say where the lines came from, by `entire
    /// why`: the agent session, checkpoint and prompt that wrote them, or the
    /// commits people made them in. Needs Entire enabled in the repository.
    /// Off by default because `entire why` takes from tens of milliseconds
    /// to two minutes where the history is long ([entire] why_timeout_ms)
    #[usage(long)]
    pub(crate) why: bool,
    /// Print a map of the repository's most important definitions, from the
    /// code graph
    #[usage(long)]
    pub(crate) map: bool,
    /// The map's size in tokens, estimated at 4 bytes each
    #[usage(long, value_name = "TOKENS")]
    pub(crate) budget: Option<usize>,
    /// For the map: a file you are working on, or an identifier you care
    /// about. Repeatable
    #[usage(long, var, value_name = "FILE|NAME")]
    pub(crate) focus: Vec<String>,
}

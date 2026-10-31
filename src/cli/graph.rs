use usage_rs::{Args, Subcommands};

/// Query the code graph: symbols, callers, impact and affected tests
///
/// The graph is built with tree-sitter from Rust, Python, TypeScript and
/// JavaScript, Go, Ruby, Java, C and C++ files, and kept in a database in
/// $XDG_CACHE_HOME/jouet/graph, one per work tree: nothing is written into
/// the repository. There is no daemon: every query first
/// re-indexes the files that changed since the last one, and jouet's
/// post-tool-use hook re-indexes files an agent edits. The code graph
/// builds on codegraph's design and extraction.
#[derive(Args)]
pub(crate) struct GraphArgs {
    #[usage(subcommand)]
    pub(crate) cmd: GraphCmd,
}

#[derive(Subcommands)]
pub(crate) enum GraphCmd {
    /// Build the graph, or bring it up to date
    Index(GraphIndexArgs),
    /// Show where the graph lives and what it holds
    Status(GraphStatusArgs),
    /// Find symbols by name, qualified name or words
    Symbols(GraphSymbolsArgs),
    /// List what calls, constructs or names a symbol
    Callers(GraphTraverseArgs),
    /// List what a symbol calls, constructs or names
    Callees(GraphTraverseArgs),
    /// List everything a change to a symbol could break
    Impact(GraphTraverseArgs),
    /// Find the code that answers a question, with its source and neighbours
    Explore(GraphExploreArgs),
    /// List the test files that depend on changed files
    Affected(GraphAffectedArgs),
    /// Delete the graphs of worktrees that no longer exist
    Prune(GraphPruneArgs),
}

/// Delete the graphs of worktrees that no longer exist
///
/// Each worktree's graph records its root. This deletes every graph in
/// $XDG_CACHE_HOME/jouet/graph whose root is gone, and keeps one whose root
/// exists or cannot be checked. Querying a graph also does this, at most
/// once a day.
#[derive(Args)]
pub(crate) struct GraphPruneArgs {}

/// Build the graph, or bring it up to date
///
/// Lists the work tree's source files (honouring .gitignore), and extracts
/// only the files whose content changed since the last index. Files larger
/// than 1 MiB or not UTF-8 are recorded without symbols. A worktree with no
/// graph yet starts from a copy of the graph of a sibling worktree of the
/// same repository, the one whose HEAD is nearest, so only the files that
/// differ are extracted.
#[derive(Args)]
pub(crate) struct GraphIndexArgs {
    /// Drop the graph and index everything from scratch
    #[usage(long)]
    pub(crate) full: bool,
}

/// Show where the graph lives and what it holds
///
/// Prints the database's path and size, and how many files, symbols,
/// edges and references it holds. Does not index.
#[derive(Args)]
pub(crate) struct GraphStatusArgs {}

/// Find symbols by name, qualified name or words
///
/// An exact name comes first, then a qualified name (`Client::push`,
/// `Client.push`), then symbols whose name, docstring or signature has
/// every word (`retry backoff`; `processPayment` has the words `process`
/// and `payment`). With no QUERY, lists every symbol under --path. Each
/// line is `path:line kind qualified-name`, and the ID after `#` names the
/// symbol to other graph commands.
#[derive(Args)]
pub(crate) struct GraphSymbolsArgs {
    /// A name, a qualified name, or words
    pub(crate) query: Option<String>,
    /// Only symbols of this kind: function, method, class, struct, …
    #[usage(long, value_name = "KIND")]
    pub(crate) kind: Option<String>,
    /// Only symbols in this file or directory
    #[usage(long, value_name = "PATH")]
    pub(crate) path: Option<String>,
    /// The most bytes to print (default 6000; 0 is no limit)
    #[usage(long, value_name = "BYTES")]
    pub(crate) budget: Option<usize>,
    /// Which page of a longer answer to print (default 1)
    #[usage(long, value_name = "N")]
    pub(crate) page: Option<usize>,
}

/// A symbol's callers, callees or impact
///
/// SYMBOL is a name, a qualified name (`Client::push`), `PATH#NAME`,
/// `PATH:LINE` (the innermost symbol there) or an ID from `jouet graph
/// symbols`. A name that several symbols share answers for each of them.
/// callers and callees follow calls, constructions and references; impact
/// follows every edge but containment, from the symbol and everything it
/// contains, and groups the answer by file.
#[derive(Args)]
pub(crate) struct GraphTraverseArgs {
    /// The symbol
    pub(crate) symbol: String,
    /// How many hops to follow (default 1, and 3 for impact)
    #[usage(long, value_name = "N")]
    pub(crate) depth: Option<u32>,
    /// The most bytes to print (default 6000; 0 is no limit)
    #[usage(long, value_name = "BYTES")]
    pub(crate) budget: Option<usize>,
    /// Which page of a longer answer to print (default 1)
    #[usage(long, value_name = "N")]
    pub(crate) page: Option<usize>,
}

/// Find the code that answers a question, with its source and neighbours
///
/// Ranks symbols by how well their names (split into words), qualified
/// names, docstrings and signatures match the question, puts tests after
/// the code they test unless the question is about tests, and prints each
/// with its signature, docstring, callers, callees and the start of its
/// source. The answer is paged rather than printed whole: a dense answer
/// stays in the agent's context for the rest of the session.
#[derive(Args)]
pub(crate) struct GraphExploreArgs {
    /// The question, or the names you know (`retry backoff for webhooks`)
    pub(crate) query: Vec<String>,
    /// The most bytes to print (default 6000; 0 is no limit)
    #[usage(long, value_name = "BYTES")]
    pub(crate) budget: Option<usize>,
    /// The most bytes of one symbol's source (default 1200; 0 for none)
    #[usage(long, value_name = "BYTES")]
    pub(crate) source_budget: Option<usize>,
    /// Which page of a longer answer to print (default 1)
    #[usage(long, value_name = "N")]
    pub(crate) page: Option<usize>,
}

/// List the test files that depend on changed files
///
/// Follows the graph backwards from each FILE, through imports, calls,
/// references and inheritance, to every test file that reaches it, and
/// shows the chain that does. A FILE the graph cannot speak for (not
/// indexed, or in a language jouet does not extract) is listed as unknown,
/// and the exit code is 1: run the whole suite. A fallback for languages
/// without a native tool; `jouet test --affected` prefers the native one.
#[derive(Args)]
pub(crate) struct GraphAffectedArgs {
    /// The changed files
    pub(crate) files: Vec<String>,
}

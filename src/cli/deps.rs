use usage_rs::{Args, Subcommands};

/// Check a package before adding it, or the locked dependencies, against its registry and OSV
///
/// Asks the package's registry (crates.io, npm, PyPI or the Go module
/// proxy) and OSV (osv.dev), so a dependency that does not exist, is
/// deprecated or yanked, or has a known advisory is caught before it is
/// added. Only package names and versions are sent; a package the lockfile
/// says comes from a private registry, a path or git is never sent. Answers
/// are cached under jouet's cache directory: a registry's for ten minutes,
/// OSV's for a day.
#[derive(Args)]
pub(crate) struct DepsArgs {
    #[usage(subcommand)]
    pub(crate) cmd: DepsCmd,
}

#[derive(Subcommands)]
pub(crate) enum DepsCmd {
    /// Say whether a package exists, its latest version, licence, repository and advisories
    Info(DepsInfoArgs),
    /// Check the project's locked dependencies against OSV
    Audit(DepsAuditArgs),
}

/// Say whether a package exists, its latest version, licence, repository and advisories
///
/// PKG is ECOSYSTEM:NAME[@VERSION], where ECOSYSTEM is cargo, npm, pypi or
/// go. A bare NAME takes its ecosystem from the lockfile that locks it, or
/// else from the one kind of manifest here (Cargo.toml, package.json,
/// pyproject.toml or requirements.txt, go.mod). The version checked is the
/// one given, else the one the lockfile locks, else the latest.
///
/// For a package that does not exist, crates.io's and npm's search name
/// close matches; PyPI and the Go proxy have no search. Go modules have no
/// licence in the proxy.
///
/// Exits 3 when a package does not exist (or has no such version), 1 when
/// the version checked has an advisory or is yanked or retracted, or the
/// package is deprecated, 4 when the lockfile locks it from somewhere other
/// than the public registry (its name is not sent), and 5 when a registry or
/// OSV cannot be reached.
///
/// Examples:
///   jouet deps info cargo:serde_json
///   jouet deps info npm:lodash@4.17.15
///   jouet deps info pypi:requests go:github.com/spf13/cobra
///   jouet deps info zod
#[derive(Args)]
pub(crate) struct DepsInfoArgs {
    /// The packages: ECOSYSTEM:NAME[@VERSION], or a NAME the project's
    /// lockfile or manifest places
    #[usage(required)]
    pub(crate) packages: Vec<String>,
    /// Ask the registry and OSV again rather than reading the cache
    #[usage(long)]
    pub(crate) refresh: bool,
}

/// Check the project's locked dependencies against OSV
///
/// Reads the nearest Cargo.lock, npm-family lockfile (package-lock.json,
/// pnpm-lock.yaml, yarn.lock, bun.lock), uv.lock or poetry.lock, and go.mod,
/// from here up to the repository's root, and asks OSV about every package
/// version they lock from a public registry, in batches. Lists only the
/// packages an advisory affects, each with its advisory IDs, severity and
/// the first version that fixes it.
///
/// Exits 1 when any locked package is affected, 3 when there is no
/// lockfile, and 5 when OSV cannot be reached.
///
/// Examples:
///   jouet deps audit
///   jouet deps audit --ecosystem npm
#[derive(Args)]
pub(crate) struct DepsAuditArgs {
    /// Only this ecosystem's lockfile: cargo, npm, pypi or go
    #[usage(long, value_name = "ECOSYSTEM")]
    pub(crate) ecosystem: Option<String>,
    /// Ask OSV again rather than reading the cache
    #[usage(long)]
    pub(crate) refresh: bool,
}

use usage_rs::Args;

/// Write one file for reviewing a range of commits
///
/// The file holds the commits, oldest first, a diffstat, and the full diff
/// with --context lines around each change. Refuses an empty range, and a
/// BASE that is not an ancestor of HEAD (exit 4), since the diff would then
/// show changes HEAD does not make. Written to
/// `<git dir>/jouet/review-BASE-HEAD.md` unless -o names a file (`-` is
/// stdout).
#[derive(Args)]
pub(crate) struct ReviewPackageArgs {
    /// The commit the review starts after, such as origin/main
    pub(crate) base: String,
    /// The last commit to review (default HEAD)
    pub(crate) head: Option<String>,
    /// Write the package to FILE (`-` is stdout)
    #[usage(short = 'o', long, value_name = "FILE")]
    pub(crate) output: Option<String>,
    /// Lines of context around each change (default 10)
    #[usage(long, value_name = "N")]
    pub(crate) context: Option<u32>,
}

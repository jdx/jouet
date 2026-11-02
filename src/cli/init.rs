use usage_rs::Args;

/// Inspect this project and propose jouet configuration
///
/// Reads project files and previews any supported recommendations. It does
/// not execute project commands or trust the repository. With --yes, writes
/// the proposed rules without replacing existing files; review them before
/// deciding whether to trust the repository.
#[derive(Args)]
pub(crate) struct InitArgs {
    /// Write the proposed rules after reviewing the preview
    #[usage(long)]
    pub(crate) yes: bool,
}

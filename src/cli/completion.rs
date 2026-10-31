use usage_rs::Args;

/// Print a shell completion script
///
/// The script asks this binary for each completion, so it needs nothing else
/// installed and follows whichever jouet is on PATH. Write it where your shell
/// loads completions, or source it from your shell's startup file.
#[derive(Args)]
pub(crate) struct CompletionArgs {
    /// The shell to complete for
    #[usage(
        arg,
        choices("bash", "zsh", "fish", "powershell", "pwsh", "elvish", "nu", "nushell")
    )]
    pub(crate) shell: String,
    /// Complete `joue` instead, the name routines run under
    #[usage(long)]
    pub(crate) joue: bool,
}

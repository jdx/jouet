use usage_rs::Args;

/// Answer a harness's hook
///
/// Reads the hook's JSON on stdin and runs jouet's rules. Installed by
/// `jouet setup`; never blocks the agent on an error of its own.
#[derive(Args)]
pub(crate) struct HookArgs {
    /// claude, codex, cursor, gemini or copilot
    pub(crate) harness: Option<String>,
    /// session-start, pre-tool-use, post-tool-use, post-tool-use-failure,
    /// post-edit or stop
    pub(crate) event: Option<String>,
    /// For `stop`: the hk stop hook (`hk agent stop-hook ...`) jouet runs in
    /// its place, so one hook gives one verdict
    #[usage(long, value_name = "CMD")]
    pub(crate) hk: Option<String>,
}

//! A handoff to an agent session, not a benchmark runner.

use std::fmt::Write as _;
use std::io::{self, Write as _};

pub(super) const PROMPT: &str = "Help me compare this project with and without jouet. First inspect the project and propose a representative, reproducible task, a starting commit, and an independent way to check the result. Ask me to approve the task, agent and model, number of paired runs, and time and token budget before running anything. If there is no suitable check, report correctness as unknown.\n\nRun the same task from the same starting point with and without jouet, using separate isolated checkouts and sessions. Arrange for me to start any additional sessions needed in my normal agent interface; do not copy my credentials or change my global agent configuration. Check that jouet is active in the jouet run and absent in the comparison run, and explain any differences you cannot control. Record success or failure, total and cached input tokens, output tokens, and wall time for each run when available. Do not infer missing measurements.\n\nCompare the paired results and their limitations, including failed runs, cache effects and small samples. Do not claim a saving from a task that was not correctly completed. If the results suggest jouet underperforms, offer to prepare an issue or discussion for jdx/jouet with the evidence. Let me review the exact content and approve it before posting anything.";

pub(super) fn write_prompt(out: &mut String, prompt: &str) {
    let _ = writeln!(
        out,
        "\nStart a new agent session in the project and paste this prompt:\n\n{prompt}\n"
    );
}

pub(super) fn offer() {
    eprint!("Would you like a prompt to compare this project with and without jouet? [y/N] ");
    let _ = io::stderr().flush();
    let mut answer = String::new();
    if io::stdin().read_line(&mut answer).is_ok()
        && matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes")
    {
        let mut text = String::new();
        write_prompt(&mut text, PROMPT);
        print!("{text}");
    }
}

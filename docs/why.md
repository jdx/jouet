---
description: Coding agents rebuild the same few chores by hand and get them subtly wrong. jouet replaces each with one command.
---

# Why jouet

Coding agents spend a large share of their shell calls rebuilding the same few chores by hand, and they get them subtly wrong. jouet turns each of those chores into one command.

## What agents do today

jouet was designed from a survey of about 2,600 Claude Code and Codex sessions, which ran roughly 266,000 shell commands between them. Four chores stood out:

| Chore | What agents write | What goes wrong |
|---|---|---|
| Waiting on CI | `sleep` and `until` loops around `gh pr checks` and `gh run view`: thousands of them | `gh run view --log-failed` refuses while the run is still in progress, and a conflicted PR never gets CI, so the loop waits forever |
| PR review threads | Raw `reviewThreads` GraphQL queries, then one call per reply and resolve | New comments filtered by `commit_id` instead of time, and "Fixed in abc123" replies naming a commit that was never pushed |
| Status of many PRs | `for n in …; do gh pr view $n; done` loops | `mergeable: UNKNOWN` read as an answer, and missing CI runs never traced back to a merge conflict |
| File edits | Python heredocs that call `str.replace`, often with a hand-written `assert s.count(old) == 1` | A formatter re-wrapped the line so the literal matches nothing, or an assertion aborts the script after some files were already written |

Each of these is a few lines of shell that looks right and usually works, which is why agents keep writing them and why the failures are easy to miss.

## What jouet does instead

| Command | Replaces |
|---|---|
| [`jouet edit`](/commands/edit) | Heredoc edits with hand-written `assert s.count(old) == 1`; all or nothing, never partially written |
| [`jouet wait`](/commands/wait) | `sleep`/`until` loops around `gh pr checks` and `gh run view`; reports conflicted PRs that will never get CI |
| [`jouet pr status`](/commands/pr-status) | Per-PR loops over `gh pr view`, including `mergeable: UNKNOWN` handling |
| [`jouet pr threads`](/commands/pr-threads) | Raw `reviewThreads` queries, replies and resolves |

`jouet edit`, `jouet pr status` and `jouet pr threads` have landed. `jouet wait` is being built.

Every command follows the same [conventions](/conventions), so an agent learns them once: it never prompts or needs a TTY, its output adapts to whoever is reading, `--json` prints exactly one document, nothing is truncated without saying so, and one table of exit codes separates "the answer is no" from "I refused to do that".

## What jouet is not

- **An output compressor.** Shrinking arbitrary command output is [rtk](https://github.com/rtk-ai/rtk)'s job.
- **A worktree, stacked-PR, memory or code-search tool.** Each of those already has a healthy dedicated tool.
- **Interactive.** Every command works without a TTY and never prompts.
- **Multi-host, yet.** The PR commands start with GitHub.

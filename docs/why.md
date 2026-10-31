---
description: Coding agents rebuild the same few shell chores by hand and get them subtly wrong. What jouet replaces, how, and what comes next.
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

Each of these is a few lines of shell that looks right and usually works. That is why agents keep writing them, and why the failures are easy to miss.

## Before and after

### Editing two files

An agent read `src/main.rs` and `Cargo.toml` a while ago. Since then, `Cargo.toml` moved on to version `0.1.1`.

<p class="jouet-label is-before">Before</p>

```sh
python3 - <<'EOF'
from pathlib import Path
for path, old, new in [
    ("src/main.rs", "let x = 1;", "let x = 2;"),
    ("Cargo.toml", 'version = "0.1.0"', 'version = "0.2.0"'),
]:
    s = Path(path).read_text()
    assert s.count(old) == 1, path
    Path(path).write_text(s.replace(old, new))
EOF
```

```output
AssertionError: Cargo.toml
```

The assert catches the stale version, but only after `src/main.rs` was rewritten. The tree is half edited, and the error does not say so.

<p class="jouet-label is-after">After</p>

```spec
jouet edit <<'JOUET_EDIT'
=== src/main.rs
<<<<<<< SEARCH
    let x = 1;
=======
    let x = 2;
>>>>>>> REPLACE
=== Cargo.toml
<<<<<<< SEARCH
version = "0.1.0"
=======
version = "0.2.0"
>>>>>>> REPLACE
JOUET_EDIT
```

```output
not written src/main.rs  1 edit  +1 -1
failed      Cargo.toml   1 edit
  edit 1: no match
    closest at line 3 (94% similar), - spec / + file:
    -version = "0.1.0"
    +version = "0.1.1"
0 of 2 files written
```

jouet checks every edit before it writes any file. Nothing changed, exit code `4` says the request was refused, and the closest match shows what moved.

### Waiting for a server to start

`app.log` still holds the lines from the server's last run.

<p class="jouet-label is-before">Before</p>

```sh
until grep -q 'Listening on' app.log; do
  sleep 2
done
```

The old "Listening on" line matches straight away, so the loop exits before this server is up. And if the server never gets there, the loop never ends.

<p class="jouet-label is-after">After</p>

```sh
jouet wait log app.log --match 'Listening on' --timeout 2m
```

```output
[0s] log app.log: following
[10s] log app.log: matched
MET log app.log: matched (10s)
  12:06:31 Listening on http://localhost:3000
```

Only lines written after the wait starts count. If none matches, the wait stops at `--timeout` with exit code `124`.

### Waiting on CI

<p class="jouet-label is-before">Before</p>

```sh
until gh pr checks 42; do
  sleep 60
done
```

This loop stops only when every check passes. A failing check keeps it polling, and so does a conflicted pull request, which never gets CI at all. Either way it runs until the agent's harness times it out.

<p class="jouet-label is-after">After</p>

```sh
jouet wait ci
```

`jouet wait ci` waits on the current branch's pull request, follows it when a new commit is pushed, and returns at the first failure with the failed step's log, even while the rest of the run is still going. A conflicted pull request is reported straight away, with exit code `4`. Watching the `release` check on a mise release PR, it came back three seconds in:

```output
FAIL jdx/mise#15195 @ 843cc16  0 pass, 1 fail, 0 skipped, 0 pending  (3s)

FAIL release  step "Require auto-merge on the release PR"  required
  https://github.com/jdx/mise/actions/runs/41083340820/job/124157989724
  ##[error]Enable auto-merge on this pull request to build and test the release tarballs.
  ##[error]Process completed with exit code 1.
  full log: ~/.local/state/jouet/logs/jdx-mise-job-124157989724.log
```

The `until` loop above would still be polling: a failed check never makes `gh pr checks` succeed.

## What jouet does instead

| Command | Replaces |
|---|---|
| [`jouet edit`](/commands/edit) | Heredoc edits with a hand-written `assert s.count(old) == 1`; all or nothing, never partly written |
| [`jouet search`](/commands/search) | `rg` and `grep -r` printing every match in the tree; grouped by the function each match is in, cut to a budget, and without the matches the session already saw |
| [`jouet read --map`](/commands/read) | Listing directories and reading whole files to learn a codebase; the most important definitions, sized to a token budget |
| [`jouet wait`](/commands/wait) | `sleep` and `until` loops around log files, ports, URLs and processes; and around `gh pr checks` and `gh run view` |
| [`jouet pr status`](/commands/pr-status) | Per-PR loops over `gh pr view`, including `mergeable: UNKNOWN` |
| [`jouet pr threads`](/commands/pr-threads) | Raw `reviewThreads` queries, replies and resolves |

Every command follows the same [conventions](/conventions), so an agent learns them once: it never prompts or needs a TTY, its output adapts to whoever is reading, `--json` prints exactly one document, nothing is cut without saying so, and one table of exit codes separates "the answer is no" from "I refused to do that".

## What jouet is not

- **An output compressor.** Shrinking arbitrary command output is [rtk](https://github.com/rtk-ai/rtk)'s job.
- **A worktree, stacked-PR or memory tool.** Each of those already has a healthy dedicated tool.
- **An IDE.** `jouet search` and the map read definitions with tree-sitter, not language servers, so they match names, not resolved types; there is no server to run.
- **Interactive.** Every command works without a TTY and never prompts.
- **Multi-host, yet.** The PR commands start with GitHub.

## What comes next

The commands are the first layer. The second helps agents use them without being told every time: [`jouet setup`](/setup) installs hooks for Claude Code and Codex that catch a costly pattern as the agent runs it and point to the command that replaces it, driven by [rules](/rules) you can add to.

Next is a report built from your own agent sessions that shows where their time and tokens go. It does not exist yet, and the details will change as it is built.

## The name

jouet is pronounced "zhweh" (/ʒwɛ/, one syllable, with the "zh" of "measure") and is French for "toy". Like a wind-up toy, each command does one job when you set it going, then stops and tells you how it went.

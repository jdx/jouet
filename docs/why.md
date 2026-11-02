---
description: Where coding agents lose tokens and time, what jouet does about it, and what to expect before 1.0.
---

# Why jouet

Coding agents lose a lot of each session to choices that look reasonable at the time. An agent starts a slow build after one edit, then starts it again after the next. It tries an approach that failed yesterday. It works through a release by hand that it has done a dozen times before. Nothing it learned last session carries over to this one.

In five of six Claude Code runs on a mise benchmark task, close to 90% of the wall time or more went to running commands, nearly all of it compiling. The agent built mise two or three times a run, once for a unit test and again for an e2e test. jouet is meant to make sessions like that cheaper: it records what the work costs and what went wrong, gives later sessions that history, and has commands and routines for the mechanical parts.

## How it works

[`jouet cost`](/commands/cost) records how long each build, test and lint takes. [`jouet audit`](/commands/audit) reads your Claude Code and Codex sessions and shows where the tokens and time went, such as repeated reads, polling and failed edits.

At session start, the primer names the slowest checks. The agent can then finish the edits a slow run checks before starting it, or use a narrower check while it iterates, instead of finding out how long the build takes by waiting for it.

[Self-winding](/winding) looks across sessions for a command that keeps failing, long sleeps, or the same commands run over and over. Session review reads whole sessions for the user's corrections and the effort that went nowhere, and proposes rules, mise tasks, routines and notes for AGENTS.md.

You or the agent decide what to adopt. An adopted rule steers later calls, and a task or routine gives later sessions something to run instead of retyping it. A drafted rule only warns until it is promoted, and automatic promotion is off unless you turn it on. `jouet unwind` proposes retiring rules that aren't helping.

## Slow checks

An agent usually finds out a check is slow by waiting for it. `jouet cost` keeps the timings of earlier runs in git notes. It prefers this machine's own timings, and scales timings from other machines, such as CI, by the commands timed in both places. The primer names the slow ones, and the agent can ask for the full list:

```sh
jouet cost
```

With the timings, the agent can group its edits before a slow run and skip rebuilds it doesn't need. jouet doesn't schedule the checks itself, and it can't make a run any faster. [How the estimates work →](/commands/cost)

## Routines

A skill tells an agent how to do something, and the agent still carries out each step itself. A [routine](/routines) is the procedure written as code: a short Luau program over jouet's primitives for GitHub, git, waits, edits and project tasks. The built-in `land` routine asks what a PR needs next, waits for CI if it is still running, and merges once the PR is ready.

The agent calls the routine once, and the loops and conditions run as code. A routine declares the capabilities it needs, is tested against recorded responses, and only previews its writes until it is given `--yes`. It does the same thing every session, and you can add one to jouet without writing a Rust command.

## Commands for the chores

[`jouet setup`](/setup) installs hooks for Claude Code, Codex, Cursor, Gemini CLI and Copilot. Besides recording timings and outcomes, they catch an agent hand-rolling something jouet has a command for, and name the command; `jouet agent-help` explains each one. Among them:

- A [code graph](/commands/graph) for callers, dependencies and the tests a change reaches, and [searches](/commands/search) and [reads](/commands/read) that stay within a budget and skip what the session has already seen.
- [Edits](/commands/edit) that check every replacement before writing any file, in place of Python heredocs.
- [Waits](/commands/wait) that do the polling in one call and return the failed step's log, and [`jouet test --affected`](/commands/test), which runs only the tests a change reaches.

How much these save depends on the project and the work.

## Early development

jouet is pre-1.0, and how much it helps depends on your language, framework, test runner and agent. Your project may need some configuration, a rule or routine of your own, or a fix in jouet before it works well, and you may not see fewer tokens or faster sessions straight away. Support for more ecosystems is still being worked out, and commands, flags, exit codes and JSON output may change incompatibly between releases.

To see whether it is helping, [`jouet audit`](/commands/audit) shows where your sessions' tokens and time go, and [`jouet stats`](/commands/stats) estimates what jouet saved, against your own history. Those are estimates, not measurements. The [comparative benchmarks](/benchmarks), which run the same tasks with and without jouet and compare tokens, time and whether the task got done, are still being built.

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
| [`jouet docs`](/commands/docs) | Guessing a dependency's API from memory, or reading files out of the cargo registry and `node_modules`; its signatures and docs at the version the lockfile names |
| [`jouet deps`](/commands/deps) | Adding a dependency from memory, which may not exist, be deprecated or have a known advisory; asks its registry and OSV first, and checks the locked versions against OSV |
| [`jouet db`](/commands/db) | `sqlite3 app.db "select * from …"` and `psql -c` printing every row; the schema in a few lines, and one read-only query cut to a limit, refused when it would write or reach another machine |
| [`jouet secrets scan`](/commands/secrets) | Committing or pushing an API key, token or private key unnoticed; gitleaks' rules over the files, the staged lines or the outgoing commits, before `jouet commit` commits and before a push |
| [`jouet http`](/commands/http) | `curl` piped into `jq` or `python3 -c "import json..."` to read a few fields, which says nothing when the request failed; the status and only the JSONPaths asked for, cut to a budget, with assertions that exit 1 |
| [`jouet wait`](/commands/wait) | `sleep` and `until` loops around log files, ports, URLs and processes; and around `gh pr checks` and `gh run view` |
| [`jouet pr status`](/commands/pr-status) | Per-PR loops over `gh pr view`, including `mergeable: UNKNOWN` |
| [`jouet pr threads`](/commands/pr-threads) | Raw `reviewThreads` queries, replies and resolves |

Every command follows the same [conventions](/conventions), so an agent learns them once: it never prompts or needs a TTY, its output adapts to whoever is reading, `--json` prints exactly one document, nothing is cut without saying so, and one table of exit codes separates "the answer is no" from "I refused to do that".

## The name

jouet is pronounced "zhweh" (/ʒwɛ/, one syllable, with the "zh" of "measure") and is French for "toy". Like a wind-up toy, each command does one job when you set it going, then stops and tells you how it went.

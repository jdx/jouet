---
description: How jouet helps coding agents measure costs, learn from wasted effort and carry better workflows into future sessions, and what to expect before 1.0.
---

# Why jouet

The goal of jouet is to help coding agents use fewer tokens and finish work faster by improving how they work from one session to the next. It measures what the work costs, makes expensive choices visible, and helps turn lessons from real sessions into reusable tools and guidance.

An agent can make a reasonable-looking choice and still waste most of a session. It starts a slow rebuild after one edit, then rebuilds again after the next. It repeats an approach that failed yesterday. It reasons through a procedure it has already carried out dozens of times. Better decisions need a history of what happened and a way to act on it.

## Measure, learn, reuse

1. **Measure the work.** [`jouet cost`](/commands/cost) records how long builds, tests and lints take. [`jouet audit`](/commands/audit) reads Claude Code and Codex sessions to show where tokens and time go, including repeated reads, polling and failed edits.
2. **Make the next decision better.** The session-start primer names the slowest checks from the timing history. The agent can finish the edits a slow run checks before starting it, or use a focused check while iterating. A rebuild that takes minutes is visible before the agent repeats it.
3. **Learn from what went wrong.** [Self-winding](/winding) notices recurring failures, costly habits and repeated command sequences. Session review uses user corrections and wasted effort to propose rules, tasks, routines and project guidance.
4. **Carry the lesson forward.** Adopted rules steer future calls. Tasks and routines give later sessions a procedure they can run. `jouet unwind` can propose retiring rules that are not helping.

This is a feedback loop around the agent's work. jouet supplies measurements and proposals; the agent uses them to plan, and you or the agent decide which improvements to adopt. Draft rules warn first, and automatic promotion is opt-in.

## Know what an expensive run costs

Agents often learn that a check is slow only after waiting for it. `jouet cost` keeps timings from previous runs as git notes. Its estimates prefer this machine's own history; timings shared from other machines, such as CI, can be scaled using commands timed in both places. The session primer surfaces slow checks automatically, and the agent can ask for the full list:

```sh
jouet cost
```

Knowing those costs helps the agent group edits before a slow run, choose checks appropriate to the change and avoid unnecessary rebuilds. It gives the agent evidence for planning; it does not automatically schedule checks or guarantee a faster run. [How the estimates work →](/commands/cost)

## Like skills with executable workflows

Skills give agents reusable instructions. jouet's [routines](/routines) let you or your agent put the repeatable parts into executable code: a small Luau program over primitives for GitHub, git, waits, edits and project tasks. For example, the built-in `land` routine checks what a PR needs next, waits if CI is running, then merges when it is ready.

The model calls the routine once. Its loops and conditions run as code, with declared capabilities, recorded-response tests and previews for writes. A routine gives a procedure consistent behavior across sessions, and you can extend jouet for your workflow without adding a Rust command.

## Tools that support the loop

[`jouet setup`](/setup) installs hooks for Claude Code, Codex, Cursor, Gemini CLI and Copilot. They record outcomes and steer the agent toward useful commands, which it can discover through `jouet agent-help` as needed. A few examples:

- A [code graph](/commands/graph) answers questions about callers, dependencies and the tests a change reaches. [Budgeted searches](/commands/search) and [focused reads](/commands/read) narrow exploration and remember what the session has already seen.
- [Checked edits](/commands/edit) replace Python heredocs for simple replacements and refuse stale matches before writing any file.
- [Waits](/commands/wait) handle polling in one call and return the failure log. [Affected-test selection](/commands/test) narrows a test run to what the change reaches.

These tools give the agent concrete ways to spend less context, avoid extra turns and reduce retries. They support the feedback loop, and their value depends on the project and the work.

## Early development

jouet is pre-1.0. Benefits vary by language, framework, test runner and agent harness. Your project may need configuration, a custom rule or routine, or changes to jouet itself before it works well. You may not see token savings or faster sessions right away. Broad ecosystem support is still being worked out, and commands, flags, exit codes and JSON output may change incompatibly between releases.

[`jouet audit`](/commands/audit) shows where your Claude Code and Codex sessions spend tokens and time. [`jouet stats`](/commands/stats) records jouet usage and estimates token savings against your own history. Those estimates are not measured benchmark savings. [Comparative benchmarks](/benchmarks) are still being developed to check tokens, elapsed time and task success together.

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

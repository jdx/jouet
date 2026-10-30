# jouet — a toolkit CLI for coding agents

This document specifies the first four commands: `wait`, `pr threads`, `pr status` and `edit`.

## Why

Coding agents spend a large share of their shell calls rebuilding the same few chores by hand, and they get them subtly wrong. A survey of 1,695 Claude Code sessions (146k shell commands) and 950 Codex sessions (120k commands) on this machine found:

| Chore | Claude Code | Codex | Typical failure |
|---|---|---|---|
| Waiting on CI | ~2.5k `sleep`/`until` poll loops, many blocked by the harness | 5.3k sleep-polls, 15.6k `wait` calls, 10.4k `statusCheckRollup` queries | `gh run view --log-failed` refuses while the run is in progress; a conflicted PR never gets CI and the loop waits forever |
| PR review threads | 8.1k `reviewThreads` queries, 6.1k resolves, 2.8k replies | 5.4k queries, 1.5k resolves | Filtering new comments by `commit_id` instead of time; "Fixed in abc123" naming a commit that was never pushed |
| Status of many PRs | 4.6k `for n in …; gh pr view` loops | 4.1k loops, 2.8k `mergeable` queries | `mergeable: UNKNOWN` read as a result; missing CI runs not traced to a conflict |
| File edits | 22k Python heredoc edits, 15k with hand-written `assert s.count(old)==1` | 17k uses of the built-in patch tool | A literal reformatted by rustfmt matches nothing; an assertion aborts the script after some files were written |

Existing tools cover other slices well (rtk for compressing output, worktrunk for worktrees, beads for memory, ast-grep for structural search). Nothing covers these four, so `jouet` starts here.

## Non-goals

- Compressing arbitrary command output. That is rtk's job.
- Worktrees, stacked PRs, memory, code search, or calling MCP servers. Each already has a healthy dedicated tool.
- Interactive use. Every command works without a TTY and never prompts.
- Hosts other than GitHub in v1. The PR commands sit behind a host trait so that GitLab and Forgejo can follow.

## Shared conventions

These apply to every command, so an agent learns them once.

### Output

- **Who is reading decides the style.** Each run picks one of three display modes, using the first rule that applies:
  1. An explicit flag: `--display agent|human|plain`, `--json=compact|pretty`, or `--color`.
  2. The `JOUET_DISPLAY` environment variable.
  3. **`agent`** when an AI coding agent is detected. Detection uses mise's published `mise-agent-env` crate rather than a new list. That crate reads the vendor-neutral `AI_AGENT` variable plus about 50 per-vendor variables, among them `CLAUDECODE`, `CODEX_THREAD_ID`, `CURSOR_AGENT` and `GEMINI_CLI`. It deliberately ignores look-alikes such as Coder's `CODER_AGENT_*`, and `AI_AGENT=0` turns detection off.
  4. **`human`** when stdout is a TTY.
  5. **`plain`** otherwise: pipes, CI and redirects.

  Agent detection comes before the TTY check because a terminal does not prove a human is reading: Codex can run a command under a PTY, and so can a terminal pane an agent drives. `mise-agent-env` is also what mise uses to turn off its progress bars and table truncation for agents, so mise, mbx and `jouet` all agree on who counts as an agent.
- **What each mode does:**
  - `agent`: compact ASCII text with one line per item, no colour, no spinner or progress bar, and the "never truncate silently" markers. `--json` output is minified, and `wait` prints only state changes.
  - `human`: colour, aligned tables, relative times, and live progress for `wait`. `--json` output is pretty-printed.
  - `plain`: text like `agent` mode, without colour. `--json` output is minified.
- **`--json`** prints exactly one JSON document to stdout, which includes errors. Field names are `snake_case`. Fields are added but never renamed or removed except in a new major version of `jouet`, so the tool's own version is the schema version and documents carry none. Every mode emits the same fields; only whitespace differs.
- **JSON is written for token cost.** The examples in this document are pretty-printed for reading. Compact output is minified (indentation can be a third of a pretty-printed document's tokens). Every mode leaves out fields that are `null`, `false` or empty.
- **`--jsonl`** (only on `wait`) prints one JSON event per line while the command runs, then a final `{"event":"result",...}` line identical to the `--json` document.
- **Never truncate silently.** Any list or log that is cut short ends with an explicit marker, for example `... 214 more lines (full log: ~/.local/state/jouet/logs/jdx-mise-job-123456.log)`. `--max-lines N` adjusts the budget, and `--max-lines 0` removes it.
- **Full logs are always saved** under `$XDG_STATE_HOME/jouet/logs/` (or `--log-dir`), and their paths appear in both text and JSON output.
- Diagnostics go to stderr. Only the result goes to stdout, so `jouet ... --json | jq` always works.

### Exit codes

| Code | Name | Meaning |
|---|---|---|
| 0 | `ok` | The command succeeded, or the condition was met |
| 1 | `failed` | The answer is negative: a check failed, a condition can no longer be met, or a post-edit check failed |
| 2 | `usage` | Bad arguments or a malformed edit spec |
| 3 | `not_found` | The PR, thread, file or run does not exist |
| 4 | `precondition` | Refused before doing anything: an edit did not match, a file changed underneath, a thread was already resolved, or the PR is conflicted so CI cannot run |
| 5 | `remote` | Authentication, network or rate-limit failure talking to the host |
| 124 | `timeout` | Still pending when `--timeout` expired; re-running the same command resumes waiting |
| 130 | `interrupted` | Stopped by a signal |

With `--json`, a non-zero exit also prints `{"error":{"code":"precondition","message":"...","hint":"..."}}`. The `code` string always matches the name column above.

`1` and `4` are deliberately separate. `1` says "I did the work and the answer is no", and `4` says "I refused to do the work". An agent should fix code after a `1` and fix its request after a `4`.

### Identifying a PR

A `PR` argument may be a number, a URL, `owner/repo#123` or a branch name. When it is omitted, `jouet` uses the PR for the current branch. The repository comes from `--repo owner/name`, otherwise from git remotes in the same order `gh` uses.

### GitHub authentication

Tokens are tried in this order: `JOUET_GITHUB_TOKEN`, `GH_TOKEN`, `GITHUB_TOKEN`, then `gh auth token`. The first token is checked with a cheap request, and on a 401 the next source is tried. Warnings name the source that was used ("GH_TOKEN was rejected (401); using gh auth token"). This replaces the `GH_TOKEN= GITHUB_TOKEN= gh ...` prefix that appears 8.5k times in the Claude sessions.

### Durations and time

Durations look like `90s`, `10m` or `1h30m`. Times accept RFC 3339, relative values (`2h` means two hours ago), or `@SHA`, which means "when that commit was pushed to the PR".

### Configuration

The optional config file `~/.config/jouet/config.toml` holds:

- `reply_footer`: appended to every reply `jouet` posts, such as an AI disclosure line.
- `bots`: extra logins to treat as bots, in addition to `*[bot]`.
- `default_timeout`, `log_dir` and `max_lines`.

Every setting can also be given by an environment variable (`JOUET_REPLY_FOOTER`, ...) or a flag.

## `jouet wait`

`jouet wait` blocks until a condition holds, then prints one summary. It replaces `sleep N; check; repeat` loops.

### Subjects

```
jouet wait ci [PR]       [--sha SHA] [--required] [--check GLOB]... [--ignore GLOB]...
                      [--all] [--log-lines N] [--no-logs] [--appear-timeout DUR]
jouet wait run RUN       [--log-lines N] [--no-logs]
jouet wait pr [PR]       --until mergeable|merged|closed|approved|checks-registered|ready|any-change
jouet wait file PATH     [--contains REGEX] [--gone]
jouet wait log PATH      --match REGEX [--from-start]
jouet wait port ADDR     [--closed]
jouet wait http URL      [--status CODE] [--contains REGEX]
jouet wait pid PID
jouet wait cmd -- CMD... [--expect-exit N]

common: --timeout DUR (default 30m)  --interval DUR (default 10s, backs off to 60s)
        --json | --jsonl  --quiet
```

`RUN` is a run ID or an Actions URL. `ADDR` is `host:port`, or just a port, which means localhost.

### `wait ci` behaviour

- **Which commit.** By default it follows the PR head. If the head moves while waiting (an autofix bot or another push), it switches to the new SHA, emits a `head_changed` event, and reports `head_changed: true`. `--sha` pins one commit instead.
- **Fail fast.** By default it returns as soon as any selected check fails, with exit 1. `--all` waits for every check to finish before reporting.
- **Which checks.** `--required` selects only checks required by branch protection. `--check` and `--ignore` filter checks by name glob. Skipped and neutral checks count as passing.
- **Checks that never appear.** If no checks are registered within `--appear-timeout` (default 3m), the result is `no_checks` with exit 1. If the PR is conflicted, `pull_request` workflows never run, so it returns `conflict` with exit 4 right away instead of waiting.
- **Failure logs.** For each failed GitHub Actions job, `jouet` fetches that job's log with `GET /repos/{owner}/{repo}/actions/jobs/{job_id}/logs`. That endpoint works once the job finishes, even while the rest of the run is in progress (where `--log-failed` refuses). `jouet` then:
  - finds the failed step from the job's step list;
  - cuts the log at that step's `##[group]` markers;
  - removes timestamps and ANSI escapes;
  - keeps the last `--log-lines` lines (default 60), plus any earlier lines matching `error|FAILED|panicked|assert` with two lines of context, joined by `...` markers.

  The full log is saved and its path printed. Checks that are not Actions jobs (external status checks and apps) report only their name, conclusion and details URL.
- **Polling cost.** Checks are polled over REST with `If-None-Match`, so an unchanged poll returns 304 and does not count against the rate limit. On a rate-limit response it sleeps until the reset time when that fits in `--timeout`, and otherwise exits 5.
- **Timeout.** On timeout it exits 124 with the current counts. The same command can be run again to keep waiting.
- **Shared, resumable state.** Each watcher keeps its state in `$XDG_STATE_HOME/jouet/watch/<owner>-<repo>-<pr>.json` and holds a lock on it. A second `wait` on the same PR, from another session or another agent, reads that state instead of polling GitHub a second time. A restarted `wait` replays any events it missed. This takes the place of a daemon (see "Client/server" below).
- **Base branch moved.** When new commits on the base branch touch files this PR changes, `wait` emits a `base_touched` event naming the files. It does not end the wait.

Agents should run long waits as background tasks where the harness supports that (Claude Code's `run_in_background`, so the exit notifies the agent). Otherwise they should pass a `--timeout` shorter than the harness's command timeout and re-run on exit 124.

### `wait ci` text output

```
FAIL jdx/mise#15002 @ 3f2a1bc  41 pass, 2 fail, 3 skipped, 0 pending  (6m12s)

FAIL unit (ubuntu-latest)  step "cargo nextest"  required
  https://github.com/jdx/mise/actions/runs/456/job/123456
  ---- config::tests::trust_root stdout ----
  thread 'config::tests::trust_root' panicked at src/config/mod.rs:812:9:
  assertion `left == right` failed
    left: "untrusted"
   right: "trusted"
  ...
  error: test run failed, 1 of 2210 tests failed
  full log: ~/.local/state/jouet/logs/jdx-mise-job-123456.log

FAIL lint  step "hk check"  required
  https://github.com/jdx/mise/actions/runs/456/job/123457
  src/cli/run.rs:88: missing doc comment
  full log: ~/.local/state/jouet/logs/jdx-mise-job-123457.log
```

The first word of the first line is always `PASS`, `FAIL`, `PENDING` (on timeout), `CONFLICT` or `NO_CHECKS`.

### `wait ci` JSON

```json
{
  "repo": "jdx/mise",
  "pr": 15002,
  "head_sha": "3f2a1bc9e0...",
  "head_changed": false,
  "result": "failure",
  "elapsed_s": 372,
  "counts": { "pass": 41, "fail": 2, "pending": 0, "skipped": 3, "cancelled": 0 },
  "checks": [
    {
      "name": "unit (ubuntu-latest)",
      "workflow": "test",
      "state": "fail",
      "conclusion": "failure",
      "required": true,
      "url": "https://github.com/jdx/mise/actions/runs/456/job/123456",
      "run_id": 456,
      "job_id": 123456,
      "failed_step": "cargo nextest",
      "log_excerpt": ["---- config::tests::trust_root stdout ----", "..."],
      "log_truncated": true,
      "log_path": "/home/coder/.local/state/jouet/logs/jdx-mise-job-123456.log"
    }
  ]
}
```

- `result` is one of `success`, `failure`, `timeout`, `conflict` or `no_checks`.
- `checks` lists every selected check. The `log_*` fields appear only on failed checks.
- `--jsonl` adds these events: `check_registered`, `check_completed`, `head_changed` and `rate_limited`.

### Other subjects

- **`wait pr --until mergeable`** keeps polling while GitHub reports `UNKNOWN` (it is still computing). It exits 0 when the PR is `MERGEABLE` and 1 when it is `CONFLICTING`, so the agent never has to interpret `UNKNOWN`.
- **`wait pr --until merged`** exits 1 if the PR is closed without merging.
- **`--until approved`** exits 1 when changes are requested.
- **`--until checks-registered`** returns as soon as any check exists for the head commit. Use it after pushing, before `wait ci`.
- **`--until ready`** waits until the PR has no `attention` reasons (the `pr status` codes), and exits 1 as soon as one of them can only be cleared by the agent: `conflict`, `checks_failing`, `changes_requested` or `unresolved_threads`.
- **`--until any-change`** returns on the next change of any kind (a push, a check finishing, a review, a comment or a thread) and prints that one event. It is meant for an agent babysitting a PR. Under `--jsonl` with Claude Code's Monitor tool, every line becomes a notification.
- **Local subjects** exit 0 when the condition holds and 124 on timeout.
  - `wait log` follows the file across rotation, like `tail -F`, and starts at the current end unless given `--from-start`. It prints the matching line.
  - `wait cmd` re-runs the command every `--interval` until it exits with `--expect-exit` (default 0), and prints the last run's output tail.

These local subjects deliberately stay small. Long-running services belong in pitchfork, whose ready checks cover the daemon case.

### `wait` exit codes

`0` met · `1` can no longer be met (a check failed, the PR was closed, changes were requested) · `3` PR or run not found · `4` conflicted PR (no CI will run) · `5` remote · `124` timeout

## `jouet pr threads`

`jouet pr threads` lists, replies to and resolves review threads. It replaces hand-written `reviewThreads` GraphQL and per-comment reply calls.

### Handles

Every thread is addressed by the database ID of its first comment, a plain number (for example `4377519031`) that is unique across GitHub and shown in the web UI's comment links. `jouet` maps it to the GraphQL thread ID that resolving needs and to the comment ID that replying needs, so an agent handles one number per thread.

### Listing

```
jouet pr threads [PR] [--all | --resolved] [--outdated | --no-outdated]
                   [--author LOGIN]... [--bots | --no-bots]
                   [--since TIME] [--path GLOB]... [--include-reviews]
                   [--context N] [--full] [--fail-if-unresolved]
```

- **Default selection.** Unresolved threads only, including outdated ones (they are marked).
- **`--since TIME`** keeps threads that have any comment newer than `TIME`. It filters by comment creation time, never by the comment's `commit_id`: bots post comments on the commit they reviewed, which can be older than comments posted after it. `--since @SHA` means "anything posted after SHA was pushed", which is the usual "what did the bots say about my last push" query.
- **`--author` and `--bots`** filter by who started the thread. Logins ending in `[bot]` and those in the config's `bots` list count as bots.
- **`--include-reviews`** also lists review summary bodies and PR conversation comments in the same window. They appear as `kind: "review"` or `kind: "comment"` and cannot be resolved.
- **Condensed bodies.** By default each comment body is condensed:
  - HTML comments and badge images are removed;
  - `<details>` blocks collapse to their `<summary>` line;
  - each comment is capped at 30 lines, with a truncation marker.

  Suggestion blocks are always kept in full. `--full` shows bodies unaltered.
- **`--context N`** adds N lines of the current file around the commented line, from the working tree when it is at the PR head and from the API otherwise.

### Listing text output

~~~
jdx/mise#15002  3 unresolved of 11 threads (1 outdated)  since @3f2a1bc

[4377519031] src/backend/vfox.rs:212  coderabbitai[bot]  2h ago  2 comments
  The header download ignores http_download_timeout, so a stalled server hangs
  `mise lock` indefinitely.
  suggestion:
  ```suggestion
  let resp = client.get(url).timeout(settings.http_download_timeout()).send()?;
  ```
  > jdx (1h ago): good catch, is this also true for the archive download?

[4377522210] src/lockfile/generate.rs:88  cursor[bot]  40m ago  outdated
  ...
~~~

### Listing JSON

```json
{
  "repo": "jdx/mise",
  "pr": 15002,
  "counts": { "unresolved": 3, "resolved": 8, "outdated": 1 },
  "threads": [
    {
      "id": 4377519031,
      "node_id": "PRRT_kwDOAbc...",
      "kind": "thread",
      "path": "src/backend/vfox.rs",
      "line": 212,
      "original_line": 205,
      "outdated": false,
      "resolved": false,
      "author": "coderabbitai[bot]",
      "is_bot": true,
      "url": "https://github.com/jdx/mise/pull/15002#discussion_r4377519031",
      "comments": [
        {
          "id": 4377519031,
          "author": "coderabbitai[bot]",
          "created_at": "2026-10-30T11:02:14Z",
          "body": "The header download ignores ...",
          "body_truncated": false,
          "suggestion": "let resp = client.get(url)..."
        }
      ]
    }
  ]
}
```

List exit codes: `0` (even when unresolved threads exist) · `1` only with `--fail-if-unresolved` and at least one unresolved thread · `3` · `5`

### Replying and resolving

```
jouet pr threads reply   ID (--body TEXT | --body-file FILE) [--resolve] [PR]
jouet pr threads resolve ID... [--reply TEXT] [PR]
jouet pr threads unresolve ID... [PR]
jouet pr threads apply   (--file FILE | -) [--dry-run] [PR]
```

`apply` takes a list, so a whole review sweep is posted in one call:

```json
[
  { "id": 4377519031, "reply": "Bounded by http_download_timeout in {sha}.", "resolve": true },
  { "id": 4377522210, "resolve": true },
  { "id": 4377530001, "reply": "Keeping this: the lock path must not retry, see #14990." }
]
```

Rules for every write:

- **Validate first, then write.** Every ID is looked up and checked before anything is posted. An unknown ID, or a thread that is already resolved, fails the whole call with exit 3 or 4 and posts nothing. `--ignore-resolved` skips resolved threads instead of failing.
- **`{sha}` placeholder.** `{sha}` in a reply expands to the short local HEAD SHA. If a reply uses `{sha}`, `jouet` first checks that HEAD is on the PR's head branch on the remote, and refuses with exit 4 otherwise. That rules out "Fixed in abc123" pointing at an unpushed commit.
- **Footer.** The configured `reply_footer` is appended to every reply.
- **Empty bodies.** An empty `--body` or an empty `--body-file` is a usage error (exit 2), never an empty reply.
- **Partial failure.** If a network error interrupts a batch, the output lists what was posted and what was not, and the exit code is 5. Re-running `apply` with the same file skips threads that are already resolved and replies whose exact text is already the thread's last comment from the same user, so a retry does not double-post.

Write output, in text and JSON, is one line per thread with `replied`, `resolved`, `skipped` or `failed`, and the reply URL.

Write exit codes: `0` all applied · `2` · `3` · `4` · `5`

## `jouet pr status`

`jouet pr status` gives one line per PR, with the reasons each one needs attention. It replaces `for n in ...; gh pr view` loops.

### Command

```
jouet pr status [PR...] [--mine] [--search QUERY] [--repo OWNER/NAME]...
                     [--settle DUR] [--fail-on-attention]
```

- **Selecting PRs.** With no arguments it shows the PR for the current branch. `--mine` selects open PRs authored by the token's user (across every `--repo`, or the current repo). `--search` takes a GitHub search query. Arguments and selectors can be combined.
- **One request.** All PRs are fetched in a single aliased GraphQL query (in batches of 50), so twenty PRs cost one round trip instead of twenty.
- **`--settle DUR`** (default `20s`) re-queries PRs whose mergeable state is `UNKNOWN` until GitHub settles it or the time runs out. `--settle 0` disables this.

### Fields and attention

Each PR reports:

- number, title, state (`open`, `draft`, `merged` or `closed`), base and head branches, short head SHA and when it was last pushed;
- `mergeable` (`mergeable`, `conflicting` or `unknown`) and `merge_state` (GitHub's `mergeStateStatus`, lowercased);
- check counts and failing check names;
- review decision, unresolved thread count, and whether auto-merge is enabled.

`attention` is a list of short, fixed codes derived from those fields, with a human message for each. This is the part that saves the agent from re-deriving what is wrong:

| Code | When |
|---|---|
| `conflict` | `mergeable` is `conflicting`; the message notes that `pull_request` CI will not run |
| `checks_failing` | Any check failed; the message names them |
| `checks_missing` | No checks registered more than 5 minutes after the last push |
| `changes_requested` | Review decision is `CHANGES_REQUESTED` |
| `unresolved_threads` | Unresolved review threads exist |
| `behind_base` | `merge_state` is `behind` |
| `draft` | The PR is a draft |
| `mergeable_unknown` | Still `unknown` after `--settle` |

### Text output

```
#15002 open   fix(vfox): bound header lock downloads...  CONFLICT  ci 41/46 fail: lint, unit  threads 3  review -
#15003 open   docs: describe current trust rules         ok        ci 46/46                   threads 0  review approved
#15004 draft  perf(brew): resolve from bulk index         ok        ci 12/46 pending           threads 0  review -
3 PRs, 1 needs attention: #15002 (conflict, checks_failing, unresolved_threads)
```

Titles are cut to 40 characters, with `...`. The summary line is always last.

### JSON

```json
{
  "prs": [
    {
      "repo": "jdx/mise",
      "number": 15002,
      "title": "fix(vfox): bound header lock downloads by http_download_timeout",
      "url": "https://github.com/jdx/mise/pull/15002",
      "state": "open",
      "base": "main",
      "head": "claude/vfox-timeouts-1a2b3c",
      "head_sha": "3f2a1bc9e0...",
      "pushed_at": "2026-10-30T10:55:01Z",
      "mergeable": "conflicting",
      "merge_state": "dirty",
      "checks": { "pass": 41, "fail": 2, "pending": 0, "skipped": 3, "failing": ["lint", "unit (ubuntu-latest)"] },
      "unresolved_threads": 3,
      "auto_merge": false,
      "attention": [
        { "code": "conflict", "message": "Conflicts with main; pull_request workflows will not run until it is rebased" },
        { "code": "checks_failing", "message": "2 failing: lint, unit (ubuntu-latest)" },
        { "code": "unresolved_threads", "message": "3 unresolved review threads" }
      ]
    }
  ],
  "not_found": []
}
```

### Exit codes

`0` · `1` only with `--fail-on-attention` and at least one PR needing attention · `3` if any requested PR was not found (the others are still reported, and the missing ones are listed in `not_found`) · `5`

## `jouet edit`

`jouet edit` makes exact, checked, all-or-nothing text edits across files. It replaces Python heredoc scripts with hand-written `assert s.count(old) == 1`, and offers the same patch format Codex uses, to any agent.

### Commands

```
jouet edit [--spec FILE | -] [--format blocks|patch|json] [--dry-run] [--diff]
        [--check CMD] [--revert-on-fail] [--expect-hash PATH=SHA256]...
jouet edit replace PATH --old TEXT --new TEXT [--count N | --all]
jouet edit hash PATH...
```

`jouet edit` reads a spec from `--spec` or stdin. `--format` is detected from the first non-blank line when omitted. `jouet edit replace` is a shortcut for one edit given on the command line.

### Spec formats

**Blocks** (default). Search and replace blocks, the format most models already produce reliably:

```
=== src/backend/vfox.rs
<<<<<<< SEARCH
        let resp = client.get(url).send()?;
=======
        let resp = client
            .get(url)
            .timeout(settings.http_download_timeout())
            .send()?;
>>>>>>> REPLACE

=== src/lockfile/generate.rs
<<<<<<< SEARCH count=2
use crate::http::HTTP;
=======
use crate::http::HTTP_FETCH;
>>>>>>> REPLACE
```

- `=== PATH` starts a file section.
- Each block replaces `count` matches of the search text, exactly 1 by default; `count=all` replaces every match but still needs at least one.
- An empty search text in a section for a file that does not exist creates the file.
- If the file being edited itself contains conflict markers, the fence can be lengthened: a block opened with `<<<<<<<<<< SEARCH` (10 characters) must be closed by `>>>>>>>>>> REPLACE`.

**Patch.** The `*** Begin Patch` / `*** Update File:` / `@@` / `-` and `+` lines / `*** Add File:` / `*** Delete File:` / `*** Move to:` / `*** End Patch` format used by OpenAI Codex's `apply_patch`, accepted as-is.

**JSON.** For specs generated by programs:

```json
[{ "path": "src/a.rs", "old": "x", "new": "y", "count": 1 }]
```

### Semantics

- **All or nothing.** Every edit in every file is applied in memory first. If any edit fails, no file is written and the output says so plainly ("0 of 3 files written"). Writes go to a temporary file followed by a rename, and keep the file's mode.
- **Order.** Edits to one file apply in the order given, and each sees the result of the previous one.
- **Exact matching.** Search text matches exactly, including whitespace. If the file uses CRLF line endings, LF in the search and replace text is treated as CRLF. Trailing newlines in a block are significant.
- **Diagnostics instead of guesses.** When an edit fails, `jouet` never applies a near match. Instead it reports:
  - for zero matches, the closest region in the file (by line similarity) with a short diff against the search text. This shows at a glance when a formatter re-wrapped the line or whitespace differs.
  - for too many matches, the line number of every match, so the agent can add context or set `count`.
- **Stale guard.** `--expect-hash PATH=SHA256` refuses (exit 4) if the file changed since the agent read it. `jouet edit hash PATH...` prints the hashes to pass back later.
- **Check and revert.** `--check CMD` runs a command after writing, such as `cargo check -q`. With `--revert-on-fail`, a failing check restores every edited file to its previous contents byte for byte, and the exit code is 1. This supports mutation testing: an edit that does not apply or does not compile can never be mistaken for a surviving mutation.
- **Heredoc safety.** A spec that ends inside an open block or file section (for example, when a heredoc delimiter appeared inside the content and cut the spec short) is a usage error, not a partial edit. The documentation recommends quoted, unique delimiters such as `<<'JOUET_EDIT'`.

### Text output

```
ok  src/backend/vfox.rs        1 edit   +4 -1
ok  src/lockfile/generate.rs   1 edit   +2 -2  (2 matches)
2 of 2 files written
```

When an edit does not match:

```
no match  src/backend/vfox.rs  block 1
  closest at line 212 (93% similar):
  -        let resp = client.get(url).send()?;
  +        let resp = client.get(&url).send()?;
0 of 2 files written
```

`--diff` adds a unified diff of each written file. `--dry-run` shows the same report without writing anything.

### JSON

```json
{
  "applied": false,
  "files_written": 0,
  "files": [
    {
      "path": "src/backend/vfox.rs",
      "status": "failed",
      "edits": [
        {
          "index": 0,
          "status": "no_match",
          "expected": 1,
          "found": 0,
          "closest": { "line": 212, "similarity": 0.93, "diff": "-        let resp = ...\n+        let resp = ..." }
        }
      ]
    },
    {
      "path": "src/lockfile/generate.rs",
      "status": "not_written",
      "edits": [{ "index": 0, "status": "ok", "expected": 2, "found": 2, "lines": [3, 41] }]
    }
  ]
}
```

- File `status` is `written`, `not_written` (valid but not written because another file failed), `failed`, `created`, `deleted` or `reverted`.
- Edit `status` is `ok`, `no_match`, `too_many_matches` or `too_few_matches`.
- `check` appears only when `--check` was given, and reports the command, exit code, output excerpt and log path.

### Exit codes

`0` applied (and the check passed) · `1` the check failed (files reverted with `--revert-on-fail`, kept otherwise) · `2` malformed spec · `3` a file to update or delete does not exist · `4` a match count was wrong or a file hash did not match, so nothing was written

## Agent integration

- **`jouet agent-help [COMMAND]`** prints a short, current usage guide written for agents, so `AGENTS.md` and skills can say "run `jouet agent-help`" instead of copying flags that go stale.
- **CLI definition.** The CLI is defined with usage, so shell completions and the docs are generated from one source.
- **Hooks (milestone 4).** `jouet hook claude` and `jouet hook codex` print a hook configuration that turns away the patterns `jouet` replaces and names the replacement:
  - `sleep N` followed by `gh pr checks` or `gh run view` suggests `jouet wait ci`;
  - raw `reviewThreads` GraphQL suggests `jouet pr threads`;
  - a `python3` heredoc calling `.replace(` and `write` suggests `jouet edit`.

  The hook only advises; it never rewrites a command silently.

## Implementation notes

- Rust, one static binary, no runtime dependencies. It never shells out to `gh` except to read a token.
- GitHub access goes through a small client: GraphQL for threads and PR status, REST for check runs and job logs (REST supports ETags). Every request has a timeout and retries 5xx responses and secondary rate limits with jittered backoff.
- The PR commands sit behind a `ForgeHost` trait, so GitLab and Forgejo can be added without changing the CLI.
- Testing:
  - GitHub responses are recorded as fixtures and replayed by a local fake server. That covers the hard states: `UNKNOWN` mergeability, a head SHA that moves mid-wait, a job finishing while its run is in progress, a 401 from the first token source, and secondary rate limits.
  - `edit` gets property tests: a failed spec writes nothing, and a revert restores identical bytes.

## Milestones

1. **`edit` and `pr status`.** Neither waits on anything, and between them they cover the highest-volume patterns.
2. **`pr threads`** listing and `apply`, including the `{sha}` push check.
3. **`wait ci`, `wait run` and `wait pr`**, with failure-log excerpts.
4. **Local `wait` subjects**, `agent-help`, and the Claude and Codex hooks.

## Beyond v1

### What the measurements say

Every number below comes from the same session survey as "Why", plus live timings taken on 2026-10-30.

- **Where command time goes.** Claude Code commands ran for 491 hours in total:
  - cargo test, build, check and clippy plus the mise e2e suite: about 52%;
  - GitHub operations: about 25%. That includes 29 hours of plain `sleep`.
- **Where context goes.** Tool output was about 50M tokens in the Claude sessions and 134M in the Codex sessions:
  - file reads, searches and local git: 41% (Claude) and 56% (Codex);
  - GitHub output: 17% and 24%;
  - test, build and lint output: 8% (Claude truncates it with `head`) and about 10%.
- **What a GitHub call costs.** `gh` starts in 55 ms, an API round trip takes about 300 ms, and `gh pr checks` takes about 1 s.
- **How much work is concurrent.** In a typical hour, 3 sessions call `gh` (9 at the 90th percentile, 34 at most). In 361 of 5,472 hours where a given PR was polled, more than one session polled the same PR, which accounts for 14% of all polling calls. Outputs contain 374 rate-limit messages.

### Compression

The goal is less output by design, not a filter layer over arbitrary commands:

- **GitHub output** (about a fifth of context) shrinks because `pr status`, `pr threads` and `wait ci` return compact, purpose-built results instead of raw `gh` JSON and full logs.
- **Cargo output** is compressed by mbx (below), which already has structured compiler and test messages instead of text to pattern-match.
- **Everything else** stays out of scope. Generic filters are rtk's job, and the largest sink, file reads, is a harness and habit problem rather than a toolkit one.

### Performance

Rust is still the right choice, but not for the `gh` startup time it saves (about 50 ms of a 300–1000 ms call). It matters for two reasons:

- **Hooks run on every shell call.** That was 146k calls in these sessions, where a 60 ms interpreter start becomes hours and a native binary costs about 2 ms.
- **It ships as one static binary.**

The real speedups are fewer calls and fewer turns:

- `pr status` fetches 20 PRs in one aliased query (about 1 s instead of about 20 s);
- polling uses ETags, so an unchanged check returns 304 at no rate-limit cost;
- `wait` replaces `sleep` loops, so no model turn is spent asking "is it done yet";
- a failing job's log is fetched once and cached.

### Client/server

Do not start with a daemon. Follow mbx's model instead: shared state on disk, coordinated with locks. That covers the measured needs:

- removing duplicate polling (the watch state files above);
- a shared ETag cache under `$XDG_CACHE_HOME/jouet/http`;
- a machine-wide cap on concurrent GitHub reads, shared like mbx's permit pool;
- "new since I last looked" cursors per session.

The one thing only a server unlocks is push: receiving GitHub webhooks instead of polling. If that is ever worth it, `jouet serve` should run as a pitchfork daemon, and the CLI should use it when it is up and fall back to polling when it is not.

### MCP

CLI first. A CLI costs no context until it is used, and its output pipes into `jq`. MCP tool schemas sit in context on every turn. In these sessions, Codex had a GitHub MCP connector available and still used `gh` for nearly every call.

Push notifications do not need MCP either. `wait --jsonl` under Claude Code's Monitor tool already turns each event line into a notification.

Because every command already has a JSON result, `jouet mcp` can be a thin adapter later for clients without a shell, such as claude.ai or cloud sessions. It would expose the same commands with the same JSON.

### mr-boxington

For Rust, mbx is the build layer, and `jouet` should hand off to it rather than wrap cargo:

- **Builds and tests are half of agent command time.** mbx's cross-worktree cache and machine-wide scheduler are already the answer, and the scheduler is what keeps nine parallel sessions running test suites from overloading the machine. `jouet agent-help` should say so.
- **The gap is output for agents.** mbx parses compiler and test messages for its interactive display, but non-interactive runs print plain Cargo output, which is why agents pipe Cargo through `head` 27k times. An mbx agent display mode would close that gap:
  - selected automatically when `mise-agent-env` detects an agent, the same detection `jouet` uses;
  - prints failing tests with their panic excerpts, the first compiler errors, warning counts and one cache line;
  - keeps Cargo's exit code and saves the full log.

  This is the compression slice done right for Rust.
- **`mbx analyze --json`** answers "why did everything rebuild" for an agent.
- **Affected tests.** Running only the tests for crates changed since the merge base, plus the crates that depend on them, could cut the largest single cost (118 hours of `cargo test`). nextest filtersets already express this with `rdeps()`, and mbx has the unit graph. This is unverified.
- **Shared conventions.** `jouet` and mbx should share exit-code and `--json` conventions and the `agent-help` pattern, so agents learn one style.

### Ideas from published agent skills

These come from tools well-known developers have published, checked on 2026-10-30:

- **Theo Browne's tstack** (t3dotgg/tstack, 2.4k stars in its first week) ships `pr-wait`, a Python watcher that is very close to `wait pr`: one line per event for the Monitor tool, exit codes 0/1/2, a resumable state file, and a lock per PR. It is strong validation of `wait`, and the source of `any-change`, `base_touched` and the shared state above.
- **`pr next`**, after Kent C. Dodds' `ship-pr.mjs`, works through a fixed order: merged, closed, draft, conflict, behind, failing CI, pending CI, threads, approval. It returns the single next action, with exit codes 0 (done), 10 (needs the agent) and 20 (waiting). It builds on the `pr status` attention codes.
- **`edit` additions**, after Armin Ronacher's `unified-edit`: anchors to insert before or after a block, deleting a line range, and refusing to write when a file changed between the check and the write.
- **`review-package BASE HEAD`**, after obra/superpowers: writes one diff file for a reviewer, and refuses an empty range or one where BASE is not an ancestor of HEAD.
- **A git guardrail hook**, after Matt Pocock's hook and pi-mono's rules: it blocks `reset --hard`, `clean -fd`, `stash` (the stash is shared across worktrees), `add -A`, `--no-verify` and force pushes that are not `--force-with-lease`. The rules match lessons already in the memory notes.
- **`commit PATH...`**, after pi-mono and steipete: commits only the named paths and adds the trailers and the AI disclosure.
- **`handoff` / `pickup`**, after steipete and Matt Pocock: gathers git state, PR and CI status, and running processes with attach commands, for the next session.
- **`with-server`**, after Anthropic's `with_server.py`: starts servers, waits for their ports, runs a command, and always tears down. This probably belongs in pitchfork.
- **Library behaviour**, after steipete's agent rules: cap concurrent GitHub reads at 4, and fetch a failing job's log once.

## Open questions

- **Build on tstack's `pr-wait`?** It is Python and covers one PR at a time, so `jouet` would reimplement it in Rust with the same event model. Should the event names match tstack's so skills written for one work with the other?
- **Where `pr next` lives.** Should it be a separate command, or `pr status --next`?
- **mbx agent display.** Does it belong in mbx, or should `jouet check -- cargo ...` read Cargo's JSON messages itself?
- **Standalone or inside an existing tool.** `jouet` could be its own binary, or `wait` could live in pitchfork and the PR commands in a `gh` extension. A standalone binary keeps the conventions consistent; separate homes reuse existing distribution.
- **Head following in `wait ci`.** Should it follow a moved head by default, or pin the starting SHA and report `head_changed` as its own result?
- **Thread handles.** Is the first comment's database ID a good handle, or should `jouet` print short per-PR indexes (`T1`, `T2`) that are easier to type but go stale between calls?
- **Other forges in v1.** Is GitLab or Forgejo support needed before 1.0, or is GitHub-only acceptable at first?

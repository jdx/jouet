---
description: Block until CI finishes, a PR is ready, or a file, log line, port, URL, process or command is, instead of sleep-polling.
---

# jouet wait

`jouet wait` blocks until a condition holds, then prints one summary. It replaces `sleep N; check; repeat` loops, which waste a model turn on every check and are often blocked by the agent's harness.

Two traps catch those loops when they wait on CI. `gh run view --log-failed` refuses while the run is still in progress, so a failure cannot be read until every other job finishes. And a conflicted pull request never gets `pull_request` CI, so the loop waits forever. `jouet wait ci` returns on the first failure with the failed step's log, and returns `conflict` at once for a conflicted PR.

## Pick a subject

| Waiting for | Run |
|---|---|
| A pull request's checks | `jouet wait ci` |
| One GitHub Actions run | `jouet wait run 41083340820` |
| A pull request to be mergeable, merged, approved or ready | `jouet wait pr --until merged` |
| A server to log a line | `jouet wait log app.log --match 'Listening on'` |
| A port to accept connections | `jouet wait port 3000` |
| A URL to answer | `jouet wait http http://localhost:3000/health` |
| A file to appear, or to contain a line | `jouet wait file out/done.txt --contains ok` |
| A process to exit | `jouet wait pid 4242` |
| A command to succeed | `jouet wait cmd -- pg_isready -h localhost` |

Every subject takes `--timeout` (default `30m`) and exits `124` when it runs out, so running the same command again keeps waiting.

## Synopsis

```text
jouet wait ci [PR]       [--sha SHA] [--required] [--check GLOB]... [--ignore GLOB]...
                         [--all] [--log-lines N] [--no-logs] [--appear-timeout DUR]
jouet wait run RUN       [--log-lines N] [--no-logs]
jouet wait pr [PR]       --until mergeable|merged|closed|approved|checks-registered|ready|any-change
jouet wait file PATH     [--contains REGEX] [--gone]
jouet wait log PATH      --match REGEX [--from-start]
jouet wait port ADDR     [--closed]
jouet wait http URL      [--status CODE] [--contains REGEX]
jouet wait pid PID
jouet wait cmd [--expect-exit N] -- CMD...

common: --timeout DUR (default 30m)  --interval DUR (default 10s, backs off to 60s)
        --json | --jsonl  --quiet
```

The flags are listed in the [CLI reference](/cli/wait/). `PR` is a number, a URL, `owner/repo#123` or a branch, and defaults to the current branch's PR; `--repo` names the repository for a bare number or branch. `RUN` is a run ID or an Actions URL.

## Before and after

An agent restarts a server and waits for it, but `app.log` still holds the lines from the last run:

```sh
until grep -q 'Listening on' app.log; do
  sleep 2
done
```

The old "Listening on" line matches straight away, so the loop exits before this server is up. And if the server never gets there, the loop never ends. `jouet wait log` reads only what is appended after it starts, and gives up at `--timeout`:

```output
$ jouet wait log app.log --match 'Listening on' --timeout 2m
[0s] log app.log: following
[10s] log app.log: matched
MET log app.log: matched (10s)
  12:06:31 Listening on http://localhost:3000
```

## `wait ci`

```output
$ jouet wait ci 15195 --repo jdx/mise --check release --log-lines 15
[3s] ci jdx/mise#15195: head moved from e44754d to 843cc16
[3s] ci jdx/mise#15195: release fail
FAIL jdx/mise#15195 @ 843cc16  0 pass, 1 fail, 0 skipped, 0 pending  (3s)

FAIL release  step "Require auto-merge on the release PR"  required
  https://github.com/jdx/mise/actions/runs/41083340820/job/124157989724
  Run echo "::error title=Release dry run not armed::Enable auto-merge on this pull request ..."
  ##[error]Enable auto-merge on this pull request to build and test the release tarballs.
  ##[error]Process completed with exit code 1.
  full log: ~/.local/state/jouet/logs/jdx-mise-job-124157989724.log
```

- **Which commit.** It follows the PR head. When a new commit is pushed (by an autofix bot, say), it switches to that commit, emits `head_changed`, and reports `head_changed: true`. `--sha SHA` waits on one commit instead.
- **Fail fast.** It returns on the first failed or cancelled check, with exit `1`, unless `--all` is given. Skipped and neutral checks count as passing.
- **Which checks.** `--required` keeps only the checks that branch protection requires. `--check GLOB` and `--ignore GLOB` select checks by name, where `*` matches any run of characters and `?` any one character.
- **Checks that will never come.** A conflicted PR returns `conflict` with exit `4`, because `pull_request` workflows do not run until it is rebased. jouet believes a conflict once GitHub reports it again at least 10 seconds later: for a moment after a push, GitHub's API still answers for the old head, so a conflict that a rebase just fixed can look unfixed. If no checks appear on the commit within `--appear-timeout` (default `3m`), the result is `no_checks` with exit `1`. Use `wait pr --until checks-registered` right after pushing if you only want to know that CI has started.
- **Failure logs.** For each failed GitHub Actions job, jouet fetches that job's log, which GitHub serves once the job is done even while the rest of the run is in progress. It cuts out the failed step, strips timestamps and ANSI colours, and keeps the step's last `--log-lines` lines (default `60`; `0` keeps the whole step), plus earlier lines matching `error|FAILED|panicked|assert` with two lines of context. Every gap is marked `... N lines skipped`. The full log is saved once to `<log dir>/<owner>-<repo>-job-<id>.log`, and its path is printed. Checks that are not Actions jobs report their name, conclusion and URL only. `--no-logs` skips all of this.
- **Run it here.** When jactionlint is installed, each failed Actions job ends with `locally: jouet ci run WORKFLOW --check NAME`, which runs that job on this machine with [`jouet ci run`](/commands/ci), so a fix can be tried before pushing it.
- **Base branch moved.** When new commits on the base branch touch files this PR changes, a `base_touched` event names the files. The wait keeps going.

The first word of the text result is `PASS`, `FAIL`, `PENDING` (on timeout), `CONFLICT` or `NO_CHECKS`. `--json` prints `repo`, `pr`, `head_sha`, `head_changed`, `result` (`success`, `failure`, `timeout`, `conflict` or `no_checks`), `elapsed_s`, `counts` and `checks`; failed checks also carry `failed_step`, `log_excerpt`, `log_truncated`, `log_path` and, with jactionlint installed, `local`. `--jsonl` adds `check_registered`, `check_completed`, `head_changed`, `base_touched` and `rate_limited` events.

### Polling cost and shared state

- Checks are polled over GitHub's REST API with `If-None-Match`, so a poll that finds nothing new is a `304` that does not count against the rate limit.
- When the rate limit is used up, the wait sleeps until it resets if that comes before `--timeout`, and otherwise exits `5`.
- Every wait on the same PR shares one state file, `$XDG_STATE_HOME/jouet/watch/<owner>-<repo>-<pr>.json`, guarded by a lock. A wait polls GitHub only when that state is older than its own interval, so a second wait on the same PR, from another session or another agent, reads what the first one fetched instead of polling again. A restarted wait replays the events it missed for the current head, marked `replayed: true`.

::: tip Long waits from an agent
Run long waits as background tasks where the harness supports it (Claude Code's `run_in_background`, which notifies the agent when the command exits). Otherwise, pass a `--timeout` shorter than the harness's command timeout and run the command again on exit `124`.
:::

## `wait run`

`wait run RUN` waits for one GitHub Actions run. It returns when the run completes, or on its first failed job, with the same log excerpts as `wait ci`.

## `wait pr`

`wait pr [PR] --until CONDITION` waits on the pull request itself. Each condition exits `0` when it is met and `1` when it can no longer be met:

| `--until` | Met when | Exits `1` when |
|---|---|---|
| `mergeable` | GitHub reports the PR mergeable. `UNKNOWN`, while GitHub is still computing it, keeps the wait going | the PR conflicts with its base |
| `merged` | the PR is merged | it is closed without merging |
| `closed` | it is closed or merged | |
| `approved` | the review decision is approved | changes are requested |
| `checks-registered` | any check exists on the head commit | |
| `ready` | the PR has no [`pr status`](/commands/pr-status) attention codes and no pending checks | `conflict`, `checks_failing`, `changes_requested` or `unresolved_threads` appears, since only the agent can clear those |
| `any-change` | anything changes: a push, a check result, a review, the number of unresolved threads, or the PR's state | |

Every condition except `closed` counts a merged PR as met and a PR closed without merging as failed. As with `wait ci`, a conflict fails `mergeable` and `ready` only once GitHub reports it again at least 10 seconds later. `any-change` prints the one change it saw, which makes `wait pr --until any-change --jsonl` under Claude Code's Monitor tool a way to babysit a PR.

## Local subjects

- **`wait file PATH`** waits until the path exists. With `--contains REGEX`, it waits until a line of the file matches and prints that line. With `--gone`, it waits until nothing exists at the path.
- **`wait log PATH --match REGEX`** follows the file like `tail -F`, and prints the first appended line that matches. It starts at the current end of the file, so lines already there do not count unless `--from-start` is given. A file that does not exist yet is waited for and then read from its start. When the file is rotated (the path now names a different file) or truncated, it is read again from its start. A line still being written is matched too.
- **`wait port ADDR`** waits until a TCP connection to `ADDR` succeeds, or with `--closed` until every address it resolves to refuses connections. A timeout or an unreachable network says nothing about the port, so it keeps the wait going. `ADDR` is `host:port`, `[v6]:port`, or just a port, which means localhost.
- **`wait http URL`** waits until a GET of the URL answers with a 2xx status, or exactly `--status CODE`. With `--contains REGEX`, a line of the body must match too, and that line is printed. Redirects are followed unless `--status` asks for a 3xx.
- **`wait pid PID`** waits until the process exits. The process does not have to be jouet's child.
- **`wait cmd -- CMD...`** runs the command, directly rather than through a shell, every interval until it exits with `--expect-exit` (default `0`). It prints the last run's exit code and its last 20 output lines, and saves the full output of the last run under the log directory. A run still going at the timeout is stopped, though every run gets at least one interval, so `--timeout 0` runs the command once.

These subjects stay deliberately small. Starting and supervising long-running services, with their ready checks, is [pitchfork](https://pitchfork.jdx.dev)'s job.

## Timing

The first check is immediate. After that, the time between checks starts at `--interval` and grows by half again after each check that saw no change, up to 60 seconds (or `--interval`, if that is longer). It drops back to `--interval` when the state changes. No wait runs past `--timeout`, and there is one last check at it, so `--timeout 0` checks exactly once.

On timeout, jouet exits `124`. Running the same command again keeps waiting.

## Output

```output
$ jouet wait log app.log --match 'Listening on'
[0s] log app.log: missing
[4s] log app.log: matched
MET log app.log: matched (4s)
  Listening on http://localhost:3000
```

- **Progress.** For a person at a terminal, one status line on stderr is redrawn every second. For agents, pipes and CI, each change of state is one line on stderr, so a backgrounded wait's log grows only when something happens.
- **Result.** For the local subjects and `wait pr`, the first word is `MET` or `TIMEOUT` (`wait pr` also has `FAILED`). A timeout says how many checks ran. `wait ci` and `wait run` use the words described above.
- **`--json`** prints one document. For the local subjects, it holds `subject`, `target`, `result` (`met` or `timeout`), the last `state`, `elapsed_s` and `checks`, plus the subject's own fields: `line`, `status`, `exit_code`, `output`, `output_truncated` and `log_path`. Empty fields are left out.
- **`--jsonl`** prints one `{"event":"state",...}` line for each change of state (and, for the GitHub subjects, their own events), then the result document tagged `"event":"result"`. Under Claude Code's Monitor tool, each line becomes a notification.
- **`--quiet`** prints no progress and no text result, only the exit code. JSON is still printed when it is asked for.

## Exit codes

| Code | Meaning |
|---|---|
| `0` | The condition was met |
| `1` | It can no longer be met: a check failed, no checks appeared, the PR was closed, changes were requested |
| `2` | Bad arguments, such as an invalid duration or regex |
| `3` | The PR or run does not exist, or `wait cmd`'s program does not |
| `4` | The PR is conflicted, so no `pull_request` CI will run |
| `5` | Authentication, network or rate-limit failure talking to GitHub |
| `124` | Still waiting at `--timeout`; run the same command again to keep waiting |

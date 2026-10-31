---
description: Run a GitHub Actions job on this machine through jactionlint before pushing again to find out, and say whether a failure on GitHub is flaky, infrastructure or real before rerunning it.
---

# jouet ci run

`jouet ci run` runs a workflow's jobs on this machine and reports them the way [`jouet wait ci`](/commands/wait#wait-ci) reports GitHub's: one line for the run, then each failed step with the end of its output. Use it where an agent would push a commit just to see whether CI passes, or push fix after fix at a job `wait ci` reported failing.

The running is done by [`jactionlint run`](https://github.com/jdx/jactionlint/blob/main/docs/run.md). It needs no container engine and no runner image: each job gets a copy of the checkout, a throwaway home and no secrets, inside the operating system's sandbox (Landlock and seccomp on Linux, Seatbelt on macOS). jouet finds the workflow and job behind the names GitHub shows, keeps the whole transcript in a log, and cuts what it prints to what an agent needs. Install jactionlint with `mise use -g jactionlint`; without it, `jouet ci run` exits `4`.

```text
jouet ci run [WORKFLOW] [-j ID]... [--check NAME] [-e EVENT] [--log-lines N] [--secrets-from fnox] [-- JACTIONLINT_ARGS...]
```

```output
$ jouet ci run CI --check "Unit tests" --log-lines 3
FAIL jactionlint run .github/workflows/ci.yml -j test  1 pass, 1 fail  (4s)

FAIL Unit tests
  [step 2] "run the suite"  exit status 3
    error: assertion failed at src/a.rs:3
    line 1
    line 2
    ... 95 lines skipped
    line 98
    line 99
    line 100
full log: ~/.local/state/jouet/logs/ci-run-1793405018-1166474.log
```

The flags are listed in the [CLI reference](/cli/ci/run).

## Which jobs

- **`WORKFLOW`** is a file of `.github/workflows` (the extension is optional), a path, or the workflow's `name:`. It can be left out when the repository has one workflow, or when `--check` finds the job.
- **`-j ID`** runs that job and the jobs it `needs`. Repeat it for more jobs.
- **`--check NAME`** takes the name of a check as GitHub and `wait ci` show it, such as `unit (ubuntu-latest, 1.23)` or a job's `name:`, and runs the job behind it: every cell of its matrix, and the jobs it needs. A job of a reusable workflow (`caller / job`) runs its caller. When the name matches jobs in several workflows, name the workflow.
- **`-e EVENT`** is the triggering event, `push` by default.

When `jouet wait ci`, `wait run` or `pr land` reports a failed Actions job and jactionlint is installed, the report ends with the command that runs that job here:

```output
FAIL unit  step "cargo nextest"  required
  https://github.com/jdx/demo/actions/runs/900/job/123456
  ...
  full log: ~/.local/state/jouet/logs/jdx-demo-job-123456.log
  locally: jouet ci run test --check unit
```

## What it shows

- **Failed steps** with the last `--log-lines` lines of their output (default `60`; `0` shows all of it), plus earlier lines that look like errors with a little context, cut the same way as `wait ci`'s log excerpts. A failure under `continue-on-error` does not count. When a composite action fails, the step inside it that failed is shown. jactionlint's hint for the step, such as how to open the sandbox, follows it.
- **Skipped jobs and why,** such as `runs-on macos-latest needs macOS, this machine is Linux`. These are the jobs this run cannot vouch for.
- **Warnings of the run,** such as a secret the workflow reads that was not given.
- **The full log** of jactionlint's transcript, stdout and stderr in the order they arrived, in jouet's log directory.

It shows what happens on this machine, not whether GitHub will pass. Jobs for another operating system are skipped, `secrets.*` and `github.token` are empty unless passed, and `permissions`, `environment` and OIDC are not modelled. See jactionlint's [differences from GitHub](https://github.com/jdx/jactionlint/blob/main/docs/run.md#differences-from-github).

## Passing options to jactionlint

Arguments after `--` go to `jactionlint run` as they are:

```sh
jouet ci run ci -j test -- --secret TOKEN="$TOKEN" --net=proxy
```

The command line in the report shows each `--secret` value as `***`.

## Secrets from fnox

In a project that keeps its secrets in [fnox](https://fnox.jdx.dev), `--secrets-from fnox` gives the jobs the secrets they read, without putting a value on a command line or in jouet's output:

```output
$ jouet ci run ci --secrets-from fnox
PASS jactionlint run ci  1 pass  (0s)
  warning: secret GITHUB_TOKEN is not set, so ${{ secrets.GITHUB_TOKEN }} is empty (pass --secret GITHUB_TOKEN=VALUE) (jactionlint run --explain secret-unset)
  secrets from fnox: given NPM_TOKEN, STRIPE_KEY; missing GITHUB_TOKEN
full log: ~/.local/state/jouet/logs/ci-run-1793408620-2049501.log
```

1. jouet asks `jactionlint run --list --json` which jobs will run (the ones selected, and the jobs they need). It collects the names those jobs read as `secrets.NAME` or `secrets['NAME']`, in their own blocks, in the workflow's top-level `env:`, and in a local reusable workflow (`uses: ./.github/workflows/...`) one of them calls.
2. It resolves the names that the project's fnox config defines with `fnox export --format json NAME...`. If that fails, for example because a name belongs only to another profile, it resolves them one at a time.
3. It writes the values to a temporary file that only you can read (mode `0600`) and passes it as `--secret-file`. jactionlint masks every value as `***` in its transcript, and the file is deleted when the run ends.

The report lists the secrets it gave, the ones that are missing (not defined in fnox, or with no value), and the ones it could not give. `--json` has the same as `secrets`: `from`, `given`, `missing` and `multiline`.

Its limits:

- **Multi-line values** such as certificates can't go in a `--secret-file`, so they are reported and not given.
- **Only literal names are found.** A secret named in an expression, such as `secrets[format('{0}_KEY', matrix.env)]`, is missed, and so are the secrets of a remote reusable workflow.
- **`GITHUB_TOKEN` is not special.** jactionlint injects no token, so it is missing unless fnox defines it.
- **Your own `--secret-file` can't be combined** with `--secrets-from fnox` (exit 2). `--secret NAME=VALUE` after `--` still works.
- **fnox's `as_file` secrets** arrive as the path of the file fnox wrote, not the value.

## Output and exit codes

The first word of the text result is `PASS`, `FAIL`, or `ERROR` when jactionlint refused the arguments or could not start the run; then its last lines are shown instead of jobs. `--json` prints `result` (`success`, `failure` or `error`), `workflow`, `selected` (the jobs passed with `-j`), `command`, `exit_code`, `jobs` (each with `name`, `result`, `note` and `failed_steps`, which carry `label`, `name`, `note`, `hint`, `excerpt` and `excerpt_truncated`), `warnings`, `secrets` (with `--secrets-from`), `explain` (the IDs for `jactionlint run --explain`), `output`, `log_path` and `elapsed_s`. With `--display human`, the transcript also streams to stderr as it runs.

| Exit | When |
|---|---|
| `0` | Every job succeeded or was skipped. |
| `1` | A job failed. |
| `2` | jactionlint refused the arguments, a check matches jobs in several workflows, or `--secrets-from fnox` was given with a `--secret-file`. |
| `3` | No workflow or job matches the name given. |
| `4` | jactionlint is missing, has no `run` command, or could not start the run; or `--secrets-from fnox` found no fnox project, no fnox, or fnox could resolve none of the secrets. |

# jouet ci explain

`jouet ci explain` says whether each failed GitHub Actions job is **flaky**, **infrastructure** or **real**, gives the evidence for that verdict, and reruns only what a rerun can fix. An agent that reruns CI on every red check spends runs on real failures. Worse, it can "fix" a flake by changing code that was never broken. `ci explain` gives it a reason either way.

```text
jouet ci explain [PR | RUN_URL | JOB_URL] [--run ID] [--job ID] [--repo OWNER/NAME] [--rerun] [--log-lines N]
```

It takes the same targets as `jouet wait ci`. A PR is a number, URL, `OWNER/REPO#N` or branch, and defaults to the current branch's PR; for a PR it explains the latest run of each workflow on its head. `--run ID` and `--job ID` name a run or one job, and so does an Actions URL. The flags are listed in the [CLI reference](/cli/ci/explain).

```output
$ jouet ci explain --run 41193844808 --log-lines 8
FLAKY jdx/jouet  1 flaky, 9 following
  run 41193844808 ci @ e10984c4 on fix/main-build, attempt 1

FLAKY trusted / test-macos (macos)  step "Run mise run test"
  failed: a_command_is_rerun_until_it_exits_as_expected  at tests/wait.rs:436:5
  - passed on linux on this commit (trusted / test); failed on macos
  - a failure on one OS can be a real macos bug: if a rerun fails the same way on this commit, it is real
    Run mise run test
    ... 567 lines skipped
    [test:cargo] test a_log_wait_from_the_start_reads_what_is_there ... ok
    [test:cargo] test a_command_is_rerun_until_it_exits_as_expected ... FAILED
    [test:cargo] test a_log_wait_starts_at_the_end_and_follows_rotation ... ok
    ... 45 lines skipped
    [test:cargo] error: test failed, to rerun pass `-p jouet --test wait`
    Finished in 27.12s
    [test:cargo] ERROR task failed
    ##[error]Process completed with exit code 101.
  full log: ~/.local/state/jouet/ci/jdx-jouet/job-124515918002.log

cancelled after trusted / test-macos failed, with nothing of their own: trusted / release-build (x86_64-unknown-linux-musl, ubuntu-latest), trusted / release-build (aarch64-unknown-linux-gnu, ubuntu-latest), trusted / release-build (x86_64-pc-windows-msvc, windows-latest) and 6 more

rerun: allowed, every failed job is flaky or infrastructure: jouet ci explain --run 41193844808 --repo jdx/jouet --rerun
GitHub: 11 requests, 0 from the cache
```

A compile error that the default branch has too is real, but it is not the change's fault:

```output
$ jouet ci explain --run 41193332714 --log-lines 6
REAL jdx/jouet  11 real, 2 following
  run 41193332714 ci @ 95858185 on claude/graph-repo-map, attempt 1

REAL trusted / test, trusted / release-build (x86_64-unknown-linux-musl, ubuntu-latest), trusted / msrv and 8 more  step "Run mise x hk -- hk check --all"
  error[E0425]: cannot find function `lowercase_first` in this scope
  - a compile or lint error, which fails the same way on every run
  - main is broken, not this change: the same error failed ci at b768e8fd (run 41193938886), docs at b768e8fd (run 41193938852) and docs at 8e2c4ebe (run 41193864109) on main
  - main has passed since (ci at eadbb37d, run 41196232058): merge main in
    Run rustup component add rustfmt clippy
    ... 411 lines skipped
    ✔ jactionlint
      cargo-clippy – error[E0425]: cannot find function `lowercase_first` in this scope
    ... 88 lines skipped
    
    See /home/runner/work/jouet/jouet/.git/hk/output.log for full command output
    ##[error]Process completed with exit code 101.
  full log: ~/.local/state/jouet/ci/jdx-jouet/job-124514399734.log

failed after trusted / test failed, with nothing of their own: trusted / final and final

rerun: refused, a rerun repeats a real failure: trusted / test (real), trusted / release-build (x86_64-unknown-linux-musl, ubuntu-latest) (real), trusted / msrv (real) and 8 more
GitHub: 23 requests, 1 from the cache
```

## How it decides

Each failed job's log is cut to its failed step, as in `wait ci`, and read with fixed patterns. No model is involved, and the same log always gets the same reading. The patterns find:

- **failed tests:** cargo and nextest, bats, pytest, vitest, jest and `go test`, including behind a task runner's prefix such as mise's `[test:cargo]`;
- **compile and lint errors:** rustc and clippy diagnostics, rustfmt diffs, TypeScript and Go compile errors;
- **runner and network trouble:** a lost or shut-down runner, a full disk, DNS and connection failures, registry and download errors, rate limits, a service container that did not start, the cache service, and a failure of the runner's own steps (`Set up job`, `Initialize containers`).

Jobs that failed the same way are reported together and judged once:

| Verdict | When |
|---|---|
| `infrastructure` | The runner was lost or ran out of disk; these count even when tests failed too. Or a network, registry, rate-limit, service or cache failure, when no test or error explains the failure instead. |
| `real` | A compile or lint error, which fails the same way on every run. Or a test that failed the same way on every attempt on this commit. Or one that fails the same way in the latest run on the default branch. When the default branch has the failure too, the evidence says so: *main is broken, not this change*, and whether it has passed since. A compile error is compared with the default branch's failures in every workflow from around the time the run started. |
| `flaky` | The same job passed on this commit in another attempt or run. Or the failed test passed on this commit in another job, and when that job ran on another OS, the evidence names the OSes. Or the same test failed on another branch's commit, and that branch passed later. |
| `unknown` | Anything else, for example a test that failed once and has not failed in the other runs looked at. The evidence says what was checked. |

A job that failed only because another one did, such as a `needs:` job that reports the result and has no failure of its own, *follows* that job and gets no verdict. So does a job cancelled after another one failed.

A failure on only one OS can be a real bug on that OS. It counts as flaky the first time, so it gets one rerun. If the rerun fails the same way on the same commit, the next explain finds the two failures and calls it real.

## Rerunning

`--rerun` reruns the failed jobs, and the jobs that follow them, through GitHub's rerun-failed-jobs API, or that one job with `--job`, but only when every failed job is flaky or infrastructure. If any failed job is real or unknown, it reruns nothing and exits `4`, naming the jobs. Without `--rerun`, the report gives the command to run when a rerun is allowed.

The built-in rule [`ci-rerun`](/setup#steering-rules) denies `gh run rerun` (bare, `--failed` or `--job`) and `gh api` calls to the rerun endpoints, and points the agent here first.

## History, its budget and the cache

History comes from a bounded number of requests, and no list is paginated:

- the workflow's runs on the same commit, its last 50 runs, and the earlier attempts of the run;
- for a compile error, up to 3 failed runs on the default branch from around the time the run started, and its recent passing runs;
- the jobs of at most 6 other runs, and at most 8 other logs, 3 of them from jobs that passed on the same commit.

The logs of at most 20 failed jobs per run are read; any others are `unknown`. Everything fetched is kept under `~/.local/state/jouet/ci/` (`$XDG_STATE_HOME/jouet/ci/`). A finished job's log, and the job list of a finished attempt, are reused for up to 30 days. A finished run's document and the lists of runs are reused for 5 minutes. A PR, and the runs on its head, are fetched every time. Nothing is kept longer than 30 days, and the cache stays under 64 MiB, dropping the oldest files first, checked whenever it is written. Explaining the same run again costs no requests, and a log `wait ci` already saved is not fetched again.

## Output and exit codes

The first word of the text result is the worst verdict among the failed jobs: `REAL`, `UNKNOWN`, `INFRA` or `FLAKY`. It is `PASS` when nothing failed and `PENDING` when nothing has failed yet but a run is still going. `--json` prints `repo`, `pr`, `result` (`real`, `unknown`, `infrastructure`, `flaky`, `passed` or `pending`), `runs` and `rerun` (`allowed`, `reason`, `command`, `jobs`, `started`), plus `github` (`requests`, `cached`). Each run has `run_id`, `workflow`, `head_sha`, `head_branch`, `attempt`, `url`, `prs`, `in_progress`, `jobs` and `following`. Each job has `name`, `job_id`, `url`, `os`, `verdict`, `failed_step`, `tests`, `location`, `errors`, `infrastructure` (`cause`, `line`), `main_broken`, `evidence`, `log_excerpt`, `log_truncated` and `log_path`.

| Exit | When |
|---|---|
| `0` | Nothing failed, every failure is flaky or infrastructure, or `--rerun` started the rerun. |
| `1` | A failure is real or unknown. |
| `2` | More than one of a PR, `--run` and `--job` was given. |
| `3` | The PR, run or job does not exist. |
| `4` | Nothing has failed yet in a run that is still going (`jouet wait run` first), or `--rerun` refused: a failure is real or unknown, nothing failed, or a run is still in progress. |
| `5` | GitHub could not be reached, or refused the request or the rerun. |

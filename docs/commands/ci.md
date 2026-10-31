---
description: Run a GitHub Actions job on this machine through jactionlint, and see why it failed, before pushing again to find out.
---

# jouet ci run

`jouet ci run` runs a workflow's jobs on this machine and reports them the way [`jouet wait ci`](/commands/wait#wait-ci) reports GitHub's: one line for the run, then each failed step with the end of its output. Use it where an agent would push a commit just to see whether CI passes, or push fix after fix at a job `wait ci` reported failing.

The running is done by [`jactionlint run`](https://github.com/jdx/jactionlint/blob/main/docs/run.md). It needs no container engine and no runner image: each job gets a copy of the checkout, a throwaway home and no secrets, inside the operating system's sandbox (Landlock and seccomp on Linux, Seatbelt on macOS). jouet finds the workflow and job behind the names GitHub shows, keeps the whole transcript in a log, and cuts what it prints to what an agent needs. Install jactionlint with `mise use -g jactionlint`; without it, `jouet ci run` exits `4`.

```text
jouet ci run [WORKFLOW] [-j ID]... [--check NAME] [-e EVENT] [--log-lines N] [-- JACTIONLINT_ARGS...]
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

## Output and exit codes

The first word of the text result is `PASS`, `FAIL`, or `ERROR` when jactionlint refused the arguments or could not start the run; then its last lines are shown instead of jobs. `--json` prints `result` (`success`, `failure` or `error`), `workflow`, `selected` (the jobs passed with `-j`), `command`, `exit_code`, `jobs` (each with `name`, `result`, `note` and `failed_steps`, which carry `label`, `name`, `note`, `hint`, `excerpt` and `excerpt_truncated`), `warnings`, `explain` (the IDs for `jactionlint run --explain`), `output`, `log_path` and `elapsed_s`. With `--display human`, the transcript also streams to stderr as it runs.

| Exit | When |
|---|---|
| `0` | Every job succeeded or was skipped. |
| `1` | A job failed. |
| `2` | jactionlint refused the arguments, or a check matches jobs in several workflows. |
| `3` | No workflow or job matches the name given. |
| `4` | jactionlint is missing, has no `run` command, or could not start the run. |

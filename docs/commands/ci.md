---
description: Run a GitHub Actions job on this machine through jactionlint, and see why it failed, before pushing again to find out.
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

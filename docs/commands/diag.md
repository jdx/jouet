---
description: The errors and warnings in the files you changed, from the project's hk checks, and only the ones an edit introduced.
---

# jouet diag

`jouet diag` tells an agent what is wrong with the files it just changed: each finding as a file, line, column, severity, rule, message and the check that found it. It runs the project's own checks through [hk](https://hk.jdx.dev), on exactly those files, instead of the whole lint suite. `jouet diag delta` goes one step further and reports only what an edit **introduced**.

```text
jouet diag check [FILES... | --changed] [--max N]
jouet diag delta [FILES... | --changed] [--max N]
jouet diag fix   [FILES... | --changed] [--step STEP]... [--dry-run]
```

With no files, `--changed` is assumed: the files that differ from the index or HEAD, plus untracked files git does not ignore. The flags are listed in the [CLI reference](/cli/diag).

## What it runs

In a repository with an hk config (`hk.pkl`, `.config/hk.pkl`, or their `hk.local.pkl` variants, or `HK_FILE`), `jouet diag check` runs:

```sh
hk run check --safe --no-fail-fast --format json --files0-from -
```

with the files on stdin, and reads hk's normalised diagnostics from its JSON result. `JOUET_HK` names another hk binary.

- **`--safe`.** hk refuses, before any step starts, a config with a command whose [effect](https://hk.jdx.dev/agents) is unknown or destructive. jouet reports that refusal with hk's reason and exits `4`; it never reads it as a pass. Give such a step a `CommandSpec` with its `effect` to let it run.
- **Steps without diagnostics.** hk only turns a step's output into diagnostics when the step sets [`diagnostic_format`](https://hk.jdx.dev/configuration#diagnostics). A failed step without one is reported by its status and the last 20 lines of its output, with a marker saying what was left out and where hk's whole result is kept.
- **Findings outside the files.** A step that checks a whole crate or package, such as Clippy, can report files you did not name. They are kept, listed after the files you asked about.

```output
$ jouet diag check --changed
notes.txt:2:1: warning: TODO left in [todo] (todo)
step trailing failed:
  1:# Notes
1 finding in 2 files, 1 failed step (hk, 212 ms)
```

At most 50 findings are printed unless `--max` says otherwise; the rest are counted (`... 12 more findings`) and `--json` and hk's saved result have them all.

### Without an hk config

A small fallback runs the language's own checker in its JSON mode when it is on `PATH`: `cargo clippy --message-format=json` for Rust files (it checks the crate, since Clippy cannot check one file), `ruff check --output-format=json` for Python and `shellcheck --format=json1` for shell scripts. For any other file, `jouet diag` says diagnostics are unavailable and exits `4`, rather than reporting a pass.

## Only what an edit introduced

`jouet diag delta` checks the files, then compares each with the last check of an **earlier version** of it. Every check, `jouet diag check` included, remembers per file the file's SHA-256 and what was found in it (the last two versions, in `$XDG_STATE_HOME/jouet/diag/`), so the usual flow is a check before editing and a delta after.

A finding after the edit is the same one as a finding before it when the file, rule and step match and either:

- the message is the same, digits aside, at any distance: lines added above it moved it; or
- it is within three lines: the message changed with the code around it.

Pairs are matched closest first, so a new finding next to an old one cannot take its place. What is left over is new.

```output
$ jouet diag delta notes.txt
new: notes.txt:7:1: warning: TODO left in [todo] (todo)
1 new, 1 kept, 0 fixed in 1 file (hk, 198 ms)
```

A file unchanged since HEAD has nothing new by definition. A changed file that no check has seen in another version has no baseline: its findings are listed under `no_baseline` and `unknown`, and are not counted as new. A step without `diagnostic_format` that fails now, and did not before, is a new failure.

`delta` exits `1` when the edit introduced a finding or a new step failure, and `0` otherwise, even with findings that were already there.

### In `jouet edit`

`jouet edit --diag` checks the files before writing and after, and fails only on new findings; with `--revert-on-fail` it restores them. See [`jouet edit`](/commands/edit#fail-only-on-new-diagnostics-with-diag).

## Formatting after edits

Agents run formatters constantly: in one set of sessions, `cargo fmt` ran about 13,000 times and a lint-fix task averaged 20,000 characters of output, much of it formatting round trips. With

```toml
# ~/.config/jouet/config.toml
[edit]
fix_after = true
```

the project's hk fixers format each file after `jouet edit` writes it, and after the harness's own Edit, Write and MultiEdit tools change one (Claude Code's `PostToolUse` hook, which `jouet setup` installs). They run as:

```sh
hk run fix --safe --no-fail-fast --stash none --no-stage --format json --files0-from -
```

- **Safe.** `--safe` refuses a fixer whose effect is unknown or destructive, so the hook never runs a command hk cannot vouch for.
- **Bounded.** The fix is stopped after `fix_timeout_ms` (8 s by default, under the hook's 10 s), and `fix_steps = ["cargo-fmt", ...]` limits it to fast steps.
- **Reported.** The agent is told `hk fix formatted 1 file (src/a.rs); reread before editing them again`, and nothing when nothing changed. The [ledger](/setup#the-ledger) records a `fix` entry with how many files changed, never their names.

`jouet diag fix` runs the same on files by hand, and `--dry-run` says what it would run.

## Verify and one stop gate

Lint runs through hk count for [jouet's stop check](/setup#finishing-on-a-failing-check) like test runs do: `hk check`, `hk run check` and `jouet diag check` are recorded as lint runs, full with `--all` and partial otherwise. A failing one blocks finishing until a later run covers it. In a repository with an hk config, a passing check of the changed files is remembered with the tree it saw, and if files change after it, the stop hook asks for another.

### One stop gate

hk has a stop hook of its own (`hk agent stop-hook`, from `hk agent hooks`). Installed beside jouet's, both could block, with two messages. When `jouet setup` finds hk's stop hook in the same settings file, it merges the two: jouet's stop handler carries hk's command (`jouet hook claude stop --hk "hk agent stop-hook ..."`) and runs it, with hk's timeout, and gives one decision:

```json
{"decision":"block","reason":"The last test run failed (`cargo test --workspace` exited 101). ... [jouet: verify]\n\nhk check: hk check failed: ... [hk]"}
```

- hk's hook is not run when the session's last hk check passed at the tree as it is now.
- Without jouet on `PATH`, the handler runs hk's hook alone.
- `jouet setup --uninstall` puts hk's handler back as it was.
- Only a handler that is exactly `hk agent stop-hook ...` (plain words, and no keys but `type`, `command` and `timeout`) is merged. Anything else, and hk's hook in another settings file, is left alone, and setup notes that it runs separately.

## Steering

The built-in `scoped-lint` rule warns, without denying, on `cargo clippy` piped into `head` or `tail`, `eslint .`, and a bare `mise run lint`, and suggests `jouet diag check --changed` while iterating, with the full lint once at the end. It stays quiet when the work tree has nothing uncommitted, such as right after a merge, pull or checkout: there is no change to check, and that lint is the full one.

## In routines

[Routines](/routines) reach it as `jouet.diag_check{files, changed, max}` and `jouet.diag_delta{...}`, which need `fs.read`, and `jouet.diag_fix{files, changed, step}`, which needs `fs.write` and previews with `--dry-run` unless the routine was given `--yes`.

## JSON output

`jouet diag check --json`:

| Field | Meaning |
|---|---|
| `status` | `clean`, `findings`, `refused` or `unavailable` |
| `source` | `hk`, or the fallback checker that ran |
| `files` | The files checked, relative to the repository root |
| `findings[]` | `file`, `line`, `col`, `severity` (`error`, `warning`, `note` or `help`), `rule`, `message`, `source_step` |
| `omitted` | Findings left out by `--max` |
| `steps[]` | Each step's `name`, `status`, number of `findings`, and for a failed step without findings the `excerpt` of its output |
| `reason` | Why hk refused, or why diagnostics are unavailable |
| `log_path` | hk's whole JSON result |

`jouet diag delta --json` has `status` (`clean`, `introduced`, `refused` or `unavailable`), `new`, `new_failed_steps`, `kept`, `fixed`, `no_baseline` and `unknown`.

## Exit codes

| Code | When |
|---|---|
| 0 | Clean; for `delta`, nothing new |
| 1 | Findings or a failed step; for `delta`, a new finding or new step failure; for `fix`, a fix step failed |
| 2 | Bad flags, or a file outside the repository |
| 4 | hk refused to run (`--safe`, or a config it could not load), or nothing can check the files |

These are the [shared exit codes](/conventions#exit-codes).

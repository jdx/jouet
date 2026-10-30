---
description: See what jouet saved, measured from its hook ledger and estimated against your own history from before setup.
---

# jouet stats

`jouet stats` reports what jouet saved. It keeps two kinds of figure apart:

- **Measured** figures are counted from the ledger that `jouet setup`'s hooks write: jouet command runs, rule decisions and what the agent did after a denial, the test, lint and build runs the `checks` rule recorded, sessions stopped from finishing after a failed check, and failing CI logs returned as the failing step's excerpt.
- **Estimated** figures price those runs by your own Claude Code and Codex history from before setup, read the way [`jouet audit`](/commands/audit) reads it.

```text
jouet stats [--since TIME] [--harness claude|codex|all] [--session ID] [--summary] [--json]
```

The flags are listed in the [CLI reference](/cli/stats). `--since` limits the ledger entries counted; the history before setup is the same whatever it says.

## How the estimates work

A request re-reads the whole context, so a request avoided saves its whole input. For each kind of work, the history before setup gives how many requests a run of it took and how much context each request read:

| jouet command | Priced by |
|---|---|
| `jouet wait ci`, `wait run`, `wait pr` | CI waits before setup: the requests after the first in each run of CI polls |
| `jouet wait file`, `log`, `port`, `http`, `pid`, `cmd` | process waits, the same way |
| `jouet pr status` | runs of PR status reads |
| `jouet pr threads` | runs of review-thread calls |
| `jouet edit` | the share of edits that failed and were redone |

Each harness is priced by its own history, which ends at that harness's first ledger entry. Every estimate shows its basis, such as "6,450 CI waits before setup, averaging 1.7 requests each at 305k tokens per request". A kind of work with fewer than 20 runs before setup is not estimated, and the report says so instead of guessing. Time saved is not estimated yet.

## Example

From the fixture ledger in jouet's tests, whose history before setup is far too thin to estimate from:

```console
$ jouet stats
jouet stats: 14 ledger entries in 2 sessions, 2026-10-31T00:00:00Z to 2026-10-31T00:07:00Z

measured, from the ledger:
  command           harness  runs  failed  time
  jouet pr status   claude      1       0    1s
  jouet wait ci     claude      1       1    4m
  jouet edit        codex       1       0    0s
  jouet pr threads  codex       1       0    0s

  rule            deny  warn  overridden  then jouet  then other  then nothing
  python-edit        1     0           1           0           1             0
  review-threads     0     1           0           0           0             0
  sleep-poll         1     0           0           1           0             0

  checks recorded: test 1 (1 failed)
  finishing blocked after a failed check: 1
  failing CI logs returned as the failing step's excerpt: 1

estimated, against your own history before setup:
  Claude Code (1 session before 2026-10-31T00:00:00Z):
  - jouet wait ci/run/pr: 1 run, not estimated: only 1 CI wait before setup, and an estimate needs 20
  - jouet pr status: 1 run, not estimated: only 1 PR status check before setup, and an estimate needs 20
  Codex (3 sessions before 2026-10-31T00:06:40Z):
  - jouet pr threads: 1 run, not estimated: only 0 review-thread rounds before setup, and an estimate needs 20
  - jouet edit: 1 run, not estimated: only 1 edit before setup, and an estimate needs 20

Measured figures come from the ledger; estimates price them by your own history and are not measurements.
```

Without a ledger, `jouet stats` says so and shows what one run of each command would save, by your history.

## The summary line

`jouet stats --summary` prints one line about what jouet saved, or nothing when there is nothing worth saying, for a stop hook to show at the end of a session (with `--session`). Every line carries a number the ledger recorded, and a line about tokens says it is an estimate:

```text
jouet[savings]: 3 waits, zero 'is it done yet?'. about 2.4M tokens left unread (estimated).
```

The `stats_summary` key in `~/.config/jouet/config.toml`, or `JOUET_STATS_SUMMARY`, sets the style: `quips` (the default), `plain` for the same fact stated plainly, or `off` for no line. The summary reads the baseline the last full `jouet stats` saved under `$XDG_STATE_HOME/jouet/stats`, so it stays fast; until then it leaves estimates out.

## JSON

`--json` prints one document: `ledger` (where it is, whether it is there), `measured`, `baselines` (one per harness, with its runs, requests and tokens per kind of work), `estimates` for the measured runs (or `per_run` without a ledger), `estimated_input_tokens` and `scan`. An estimate that was refused has `refused` with the reason and no `input_tokens`. Fields that are zero or empty are left out.

## Exit codes

`0` with a report, including without a ledger; `2` for a usage error or a bad `stats_summary`; `1` when the ledger cannot be read.

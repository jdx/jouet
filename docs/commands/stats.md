---
description: See what jouet saved and what it cost, measured from its hook ledger and estimated against your own history from before setup.
---

# jouet stats

`jouet stats` reports what jouet saved and what it cost, and the difference. It keeps two kinds of figure apart:

- **Measured** figures are counted from the ledger that `jouet setup`'s hooks write: jouet command runs, rule decisions from each rule's current revision, the sessions each rule fired in and how many of them it fired in more than once, what the agent did after a denial, the test, lint and build runs the `checks` rule recorded, sessions stopped from finishing after a failed check, and failing CI logs returned as the failing step's excerpt.
- **Estimated** figures price those runs, and what jouet added, by your own Claude Code and Codex history from before setup, read the way [`jouet audit`](/commands/audit) reads it.

The savings are what the runs would have cost at your rates before setup. That is an assumption, not a measurement: nothing records what an agent would have done without jouet. The costs are closer to measured, since the ledger records each block, denial and failure, but pricing them is an estimate too.

```text
jouet stats [--since TIME] [--harness claude|codex|all] [--session ID] [--summary] [--json]
```

The flags are listed in the [CLI reference](/cli/stats). `--since` limits the ledger entries counted; the history before setup is the same whatever it says. The ledger keeps 30 days of [rotated files](/setup#the-ledger), so measured figures reach back about that far.

## Whether a rule changed what the agent did

A warning lets the call run and shows the agent a message, so the ledger cannot say directly whether the agent took the advice. What it can say is whether the same rule fired again later in the session. In the rules table, `sessions` counts the sessions a rule denied or warned in, and `again` those in which it did so more than once.

The table counts each rule only from its current revision: the ledger records with each decision a hash of the rule's `[[rule]]` table and script (see [the ledger](/setup#the-ledger)), and `jouet stats` compares it with the rules loaded in the directory it runs in. A rule that has changed since is counted afresh, so the table does not mix what an old version did with what the current one does. Decisions of an older revision, and those from before the ledger recorded revisions, are left out with a count, and rules the ledger names that are not loaded there (removed ones, or another repository's) are left out by name. What their denials and warnings cost is still counted. In `--json`, these are `older_decisions` and `rules_not_loaded`. A warning that fires again in most of its sessions is not changing what agents do there: its message may need rewording, or the rule may suit `deny` better, or none at all. The count is per session, so one agent repeating a command many times counts once.

`overridden` counts the same command run again soon after a deny or warning, and `then jouet`, `then other` and `then nothing` say what the agent ran next after a denial.

## The stop gate

The stop gate [observes by default](/setup#observing-or-blocking): where it would block, it records a `would-block`, one for each reason, and lets the agent finish. With `[hooks] gate = "block"` it records a `block` for each reason instead. `jouet stats` counts both by reason, and judges each by the first thing that followed it in the session, at any later turn:

- **right:** an edit (the harness's edit tools, or a `jouet edit` that succeeded) or a test, lint or build run that failed came first. The agent changed something after all, or something was still broken.
- **passed:** a test, lint or build run passed before any edit. The change was fine as it stood.
- **open:** neither has happened since.

```text
  stop gate by reason (right: an edit or a failed check came next; passed: a check passed first; open: neither yet):
    would-block failed-test: 12 (right 2, passed 7, open 3)
    would-block changed-after-pass: 5 (right 0, passed 4, open 1)
```

The reasons are `failed-test`, `failed-lint` and `failed-build` (a failed run nothing has cleared), `changed-after-pass` (files changed after the last passing test run), `hk-partial` (the last hk check covered only some files), `hk-changed` (files changed after the last hk check passed), and `hk-stop` (hk's own stop hook, which blocks in both modes). Edits made another way, such as by `sed -i`, are not seen, so an edit-only judgement undercounts. Blocks recorded before the ledger named their reasons are counted in `finishing blocked`, but not here. In `--json`, these are `measured.gate`.

## Experiment arms

Two defaults changed ahead of a controlled comparison: the [stop gate](#the-stop-gate) observes instead of blocking, and the `python-edit` rule is off, with the primer's "never Python" line. To compare old and new on your own sessions, `[experiment] control_share` percent of sessions (50 by default, an even split; `JOUET_EXPERIMENT_CONTROL_SHARE`) keep the old behaviour of each, assigned independently: a session can keep the gate blocking and still have `python-edit` off.

- **Assignment.** A session's arm comes from a hash of its id and the toggle's name, so every hook of the session agrees without keeping any state, and the same id always gets the same arm. The `session-start` ledger entry records it, as `arm: {"gate": "control", "python-edit": "new"}`. With `control_share = 0`, no arm is assigned or recorded.
- **What the control arm does.** The stop gate blocks as `[hooks] gate = "block"` would, and the primer says so. `python-edit` denies as `[rules] enable = ["python-edit"]` would, and the primer says "never Python". A setting that already turns the old behaviour on applies to every session, so both arms then behave alike.
- **The watcher.** This toggle trials new behaviour rather than keeping an old default. `[experiment] watcher_share` percent of Claude Code sessions (25 by default; `JOUET_EXPERIMENT_WATCHER_SHARE`) are in its `new` arm: their primer tells the agent to run `jouet wait pr --until needs-agent` as a background command after each push, and to end its turn until that wakes it. The desktop app's Auto-fix still runs in those sessions. jouet does not block its events, and an Auto-fix event about something the wait already reported needs no second fix. The other sessions are its `control` arm, with today's primer. Subagents, and harnesses other than Claude Code, are in neither arm. The ledger records `arm: {"watcher": "new"}`.
- **The report.** `jouet stats` compares the arms within each repository, so that a repository with long sessions in one arm does not pass for a difference between arms. A session's repository is the main checkout of the directory its transcript started in: every worktree of a repository counts as that repository, including a Claude Code worktree under `.claude/worktrees/` that has since been removed. For each toggle, it reads the transcripts of the sessions with an arm and shows each arm's n, and the median and interquartile range per session of four measures. The difference of the medians (control less new) comes with a 95% bootstrap interval, or none while an arm has fewer than two sessions. The measures are:
  - **requests:** model requests, each counted once even where a resumed session repeats them.
  - **input tokens:** over those requests, cached or not.
  - **active minutes:** tool time plus model time, as wall time. Every request of every transcript of the session goes on one timeline, from the model's wait before it (since whatever came before, at most 15 minutes, so time away from the keyboard is not counted) to the end of its calls. Time any of them covers counts once, so a resumed copy or a subagent working alongside the lead adds nothing twice.
  - **CI failures:** the desktop app's Auto-fix events that report failing checks.

```text
experiment arms, per session, within each repository (control: the old behaviour; median [interquartile range]; difference: control less new, with a 95% bootstrap interval):
  ~/src/app:
    gate: control n=2, new n=2
      requests        control 6.0 [5.5, 6.5]  new 3.0 [2.5, 3.5]  difference 3.0 [1.0, 5.0]
      sessions needed for a difference in requests to show: 30% about 10 an arm (16 more); 10% about 89 an arm (174 more)
  too few to compare (an arm with fewer than two sessions), by sessions: ~/src/lib (3)
```

A repository is shown once some toggle has two sessions in each arm; the rest are named on the last line with their session count, and sessions whose transcript gave no directory are `(repository unknown)`.

**Sessions needed.** For each toggle, the report says how many sessions an arm would need for a difference in requests of 30%, and of 10%, of the new arm's median to show. The interval's half-width shrinks with the square root of 1/control + 1/new sessions, so from the current half-width h and a target difference d, two equal arms of n sessions give an interval of half-width d when n = 2 × (h / d)² / (1/control + 1/new). "More" is what both arms together still lack. At that size a true difference of d has about even odds of an interval that leaves out zero; for a 4-in-5 chance, double it. It is an estimate from the spread seen so far, so it moves as sessions come in.

There are only tens of comparable sessions a week, so read the interval, not the difference alone. The arms are not matched on task, effort or Claude Code version; they are random by session id, which balances those only over enough sessions. In `--json`, these are `arms`: one entry per repository and toggle, with `repository`, and `needed` (`percent`, `per_arm`, `more`) on each difference.

## How the estimates work

A request re-reads the whole context, so a request avoided saves its whole input. For each kind of work, the history before setup gives how many requests a run of it took and how much context each request read:

| jouet command | Priced by |
|---|---|
| `jouet wait ci`, `wait run`, `wait pr` | not estimated: CI polling history does not establish linked, avoidable checks |
| `jouet wait file`, `log`, `port`, `http`, `pid`, `cmd` | linked runs of short, quiet checks of the same still-running process |
| `jouet pr status` | runs of PR status reads |
| `jouet pr threads` | runs of review-thread calls |
| `jouet edit` | the share of edits that failed and were redone, less `jouet edit`'s own failure rate |
| `jouet test` | the output a test, lint or build call gave the agent before setup, less what `jouet test` gave |

Each harness is priced by its own history, which ends at that harness's first ledger entry. Every estimate shows its basis. A kind of work with fewer than 20 runs before setup is not estimated, and the report says so instead of guessing. Time saved is not estimated yet.

`jouet edit` is credited only for failing less often than edits did before setup: its runs times the difference in failure rate, at that history's tokens per request, assuming each failed edit before setup was redone once. When it failed as often or more, it is credited nothing, and the extra failures are a cost. Its own failure rate comes from the runs that recorded their own exit code (see [own exit codes](#own-exit-codes)); without any, it is not estimated.

`jouet test` is credited for the output it keeps out of the context. Before setup, test, lint and build calls gave the agent some number of bytes of output on average. The credit is that average less what `jouet test` gave, over the calls that recorded their output size, at four bytes a token. The hooks record the size from Claude Code's `tool_response`, and only for a call that ran nothing but `jouet test`, since output shared with another command is not its alone. The credit counts that output once, though it stays in the context of every later request in the session, so it is a lower bound. A baseline saved before output sizes were kept asks for `jouet stats --rescan`.

### Own exit codes

The hooks see the exit code of the whole call, so a call such as `jouet edit replace ... && cargo test` that fails on its tests looks like a failed edit. To tell the two apart, every jouet command run in an agent's session (one with `CLAUDE_CODE_SESSION_ID`, `CODEX_THREAD_ID` or `JOUET_SESSION_ID` set) appends a `jouet-run` line to the [ledger](/setup#the-ledger) with the command's name and its own exit code, never its arguments. Its harness is its session's, from the hooks' lines; a run in a session the hooks never saw is left out. Turning the ledger off (`[hooks] ledger`) turns this off too.

In the table of commands, `calls failed` counts the calls that ran a command and failed, on it or on whatever else they ran, and `own runs` and `own failed` count its runs that recorded their own exit code, and its own failures among them. A check's answer, such as `jouet test` exit 1 for failing tests or `jouet wait pr` exit 124 for still waiting, is not an own failure (see [how the costs work](#how-the-costs-work)). Own runs can exceed runs: they include the jouet commands a call started itself, as a script, a test suite or another jouet command does, which the hooks never see.

## How the costs work

Each cost is priced the way the savings are, at the harness's average request before setup (its input tokens over its requests with token counts, which needs at least 20), and shows its basis:

| Cost | Counted from | Priced as |
|---|---|---|
| finishing blocked, then finished with no check passing | `verify` blocks in the ledger after which no test, lint or build passed before the session next stopped | one more request each |
| finishing held until a check passed | `verify` blocks a passing check followed before the session next stopped | not priced: a correct change needs that check to pass anyway |
| calls a rule denied | denials in the ledger | one more request each, for the call again or another |
| failed jouet runs | jouet commands that failed by their own exit code; in a session from before jouet recorded it, calls that ran nothing but jouet and failed | one more request each |
| `jouet edit` failures beyond the rate before setup | `jouet edit`'s own failure rate, over all its runs, against edits before setup | one more request for each failure past the old rate |
| extra `jouet test` output | the recorded output of `jouet test` calls, compared with average check output before setup | the excess bytes at four bytes per token, counted once |
| requests carrying the session-start primer | every request in your history after setup, read each time | the [primer](/setup#the-session-start-primer)'s size, at four characters a token, on every request |
| rule warnings | warnings in the ledger | not priced: a warning adds no request, but its text stays in the context |

One request per event is an assumption, not a measured minimum: an agent may recover from several failures in one request, or spend several requests on one failure. The ledger alone does not show how many requests an event added. A failed run whose exit code is a check's answer is not a cost, since it fails without jouet too: exit 1 (or 124, or `pr next`'s 10 and 20) from `jouet test`, `wait`, `ci`, `diag`, `secrets`, `http`, `pr status` or `pr next`. A `jouet search` exit 1 means no matches and is not priced as a failure; search usage, missing-path and other errors still are. Older hook records that did not retain the search subcommand cannot be identified as searches. A search chained with another jouet command in a record without command-specific exits also remains ambiguous and is counted as a failed call. When a call ran a check and another jouet command, as in `jouet edit replace ... && jouet test`, and exited with the check's answer, `calls failed` counts it as the check's. `jouet edit`'s failures are counted against the old rate instead, in their own line. The extra test output cost assumes the old average is comparable to these checks; it is not a measurement of what the same checks would have printed without jouet. The primer's requests include subagents', which may not read it, so that line can count high; its notes, such as the slow checks, are left out of its size.

The net is the savings less the costs, and may be negative. Extra test output is priced only when the baseline and recorded output sizes needed for the comparison are available. If a test run's output size or that baseline is missing, the net is unavailable because the output cost is unknown. When another cost cannot be priced, as when the history after setup was not read or, with `--session`, none of that session's transcripts was found, there is no net rather than one that leaves the cost out. The net is also unavailable when CI waits were used but their savings cannot be estimated. A harness with no history before setup at all, such as Cursor, can have neither its savings nor its costs priced: its costs are listed, left out of the net, and named under it. Other savings that cannot be estimated count as nothing.

The history before setup does not change, so `jouet stats` saves each harness's baseline and reuses it while that harness's first ledger entry and the jouet version stay the same, instead of reading the transcripts again. Reading them takes seconds on a large history. A harness with no ledger entries yet is always read, and `--rescan` reads them again.

Process-wait estimates need at least 20 linked opportunities in that history. A previously cached baseline that only grouped calls by category cannot support one; run `jouet stats --rescan` to rebuild it from the available transcripts. Unlinked or blocking waits are not evidence of avoidable requests.

## Example

From the fixture ledger in jouet's tests, its decisions stamped with the current rules' revisions as the tests do, whose history before setup is far too thin to estimate from, so there is no net:

```console
$ jouet stats
jouet stats: 14 ledger entries in 2 sessions, 2026-10-31T00:00:00Z to 2026-10-31T00:07:00Z

measured, from the ledger:
  command           harness  runs  calls failed  own runs  own failed  time
  jouet pr status   claude      1             0         0           0    1s
  jouet wait ci     claude      1             1         0           0    4m
  jouet edit        codex       1             0         0           0    0s
  jouet pr threads  codex       1             0         0           0    0s
  (calls failed: calls that ran it and failed, on it or on whatever else they ran; own: runs that recorded their own exit code, a check's answer not counted as failed; own runs include jouet runs a call started itself, as a script or a test does, so they can exceed runs)

  rule            deny  warn  sessions  again  overridden  then jouet  then other  then nothing
  python-edit        1     0         1      0           1           0           1             0
  review-threads     0     1         1      0           0           0           0             0
  sleep-poll         1     0         1      0           0           1           0             0
  (sessions: the sessions it denied or warned in; again: those in which it did so more than once)

  checks recorded: test 1 (1 failed)
  finishing blocked after a failed check: 1
  failing CI logs returned as the failing step's excerpt: 1

estimated savings: what these runs would have cost at your rates before setup (assumed, not measured):
  Claude Code (1 session before 2026-10-31T00:00:00Z):
  - jouet wait ci/run/pr: 1 run, not estimated: CI polling history does not establish linked, avoidable checks
  - jouet pr status: 1 run, not estimated: only 1 PR status check before setup, and an estimate needs 20
  Codex (3 sessions before 2026-10-31T00:06:40Z):
  - jouet pr threads: 1 run, not estimated: only 0 review-thread rounds before setup, and an estimate needs 20
  - jouet edit: 1 run, not estimated: only 1 edit before setup, and an estimate needs 20

estimated costs, priced the same way:
  Claude Code:
  - finishing blocked, then finished with no check passing: 1, not estimated: only 8 requests with token counts before setup, and a price needs 20
  - calls a rule denied: 2, not estimated: only 8 requests with token counts before setup, and a price needs 20
  - requests carrying the session-start primer: 0, about 0 input tokens
      basis: 0 requests in the history after setup, subagents' included, each reading the primer's ~500 tokens
  Codex:
  - rule warnings: 1, not estimated: a warning adds no request, but its text stays in the context that later requests read; not priced
  - requests carrying the session-start primer: 0, about 0 input tokens
      basis: 0 requests in the history after setup, subagents' included, each reading the primer's ~505 tokens

estimated savings 0, costs 0: no net, since a saving or cost above could not be priced.

Measured figures come from the ledger; estimates price them by your own history and are not measurements.
```

Without a ledger, `jouet stats` says so and shows what one run of each command would save, by your history.

## The summary line

`jouet stats --summary` prints one line about what jouet saved, or nothing when there is nothing worth saying, for a stop hook to show at the end of a session (with `--session`). Every line carries a number the ledger recorded, and a line about tokens says it is an estimate:

```text
jouet[savings]: 3 waits, zero 'is it done yet?'. about 2.4M tokens left unread (estimated).
```

It works out the same net as the report, reading only that session's transcripts after setup with `--session`, and never claims a saving the net does not support. When the net is below a saving it names, the line gives the net and says so ("estimated, net of jouet's costs"). When the costs are larger, the line is about them instead:

```text
jouet[cost]: jouet cost about 686k more input tokens than it saved (estimated). the helper needed help.
```

When the net is unknown, the line names no tokens.

`[stats] summary` in `~/.config/jouet/config.toml`, or `JOUET_STATS_SUMMARY`, sets the style: `quips` (the default), `plain` for the same fact stated plainly, or `off` for no line. A value jouet cannot read counts as the default, as with every [setting](/setup#settings), so a typo never breaks a stop hook. The summary reads the baseline the last full `jouet stats` saved under `$XDG_STATE_HOME/jouet/stats`, so it stays fast; until then it leaves estimates out, and names no tokens.

## JSON

`--json` prints one document: `ledger` (where it is, whether it is there), `measured` (each command with `runs`, `failed` for the calls that failed, and `own_runs` and `own_failed`; and `overhead`, the per-harness counts the costs come from), `baselines` (one per harness, with its runs, requests and tokens per kind of work, and `input_tokens` over all its requests), `estimates` for the measured runs (or `per_run` without a ledger), `estimated_input_tokens`, `after_setup` (each harness's `requests` after setup and its `primer_tokens`), `costs` (each with `kind`, `count`, `input_tokens` and `basis`, and `outside_net` for a harness the net leaves out), `estimated_cost_tokens`, `net_input_tokens` (savings less costs, negative when jouet cost more, absent when a cost could not be priced), `scan` (reading the history before setup) and `after_setup_scan`. An estimate or cost that was refused has `refused` with the reason and no `input_tokens`. Fields that are zero or empty are left out.

## Exit codes

`0` with a report, including without a ledger; `2` for a usage error; `1` when the ledger cannot be read.

---
description: See how long each build, test and lint takes here, from timings kept as git notes, so an agent can plan its runs.
---

# jouet cost

## Experimental local sequences

Opt in with `[cost] sequences = true` in your user config, or set
`JOUET_COST_SEQUENCES=1`. In an identified agent session, `jouet test` records
local start and finish events for its jobs under the jouet state directory.
For example, run both checks through `jouet test --cmd -- cargo clippy` and
`jouet test --cmd -- cargo test`, then inspect `jouet cost --sequences`.

For each pair seen at least three times, it reports the second check's median
duration and the number of pairs. Pairs must be adjacent among recorded
`jouet test` jobs in the same session and worktree, successful, non-overlapping,
and no more than 30 minutes apart. A failed or unrecognized job breaks the
sequence. An unfinished job prevents later pairs in that session from being
reported, because its end is unknown. Ordinary shell commands and other work are not recorded
by this experiment and may intervene, and cache state is unknown. These are
conditional observations, not measured savings or proof of cache reuse.

Events are local and are not added to git notes or transferred by `cost push`.
Commands use the same redaction as normal cost samples; session IDs, machine
identity and the worktree path are hashed. The files stay under the jouet state directory until
you delete them; recording stops with a warning once a file reaches 16 MiB.
Turn recording off with `JOUET_COST_SEQUENCES=0` or by
removing the setting; `--sequences` can still read previously recorded data.
The JSON output includes `sequences_recording` when `--sequences` is used.

Agents don't know which runs are slow. In five of six Claude Code runs on the mise benchmark task, close to 90% or more of the wall time went to running commands, nearly all of it compiling. It built mise two or three times a run, for a unit test and again for an e2e test. `jouet cost` shows what each check costs here, and the session-start primer names the slowest, so the agent can finish its edits before starting a slow run and check them all in one run.

Timings include recognized test runners such as `python3 -m tornado.test.runtests`, including when run through `jouet test --cmd`.

```text
jouet cost [--min DUR] [--json]
jouet cost fetch [REMOTE]
jouet cost push [REMOTE]
```

```text
$ jouet cost   # an example
mise run test          25m00s  3 runs elsewhere, × 1.4
mise run build          1m40s  6 runs here
mise run test:e2e …        40s  12 runs here
this machine takes 1.4× as long as the others, over 2 commands timed in both
```

A family such as `mise run test:e2e …` is one command with what it selected (a test name, a package) left out, once two of its commands have been timed. The session-start primer names up to four checks that take 20 seconds or more:

```text
jouet: slow checks here: `mise run test` ~25m00s, `mise run build` ~1m40s (`jouet cost` lists all). Finish the edits a slow run checks before starting it, so one run covers them.
```

## Where the timings come from

`jouet test` times each command it runs, and the hooks time a Bash call that runs one build, test or lint, as the [`checks` rule](/rules) recognizes them. Each timing is a line of JSON in a git note on HEAD under `refs/notes/jouet-cost`, which says which machine took it: a hash of its machine id, its CPU count and OS, and whether it ran on CI. The notes never touch the commits or the work tree. `[cost] record = false` in the config file, or `JOUET_COST_RECORD=0`, stops recording.

Estimates read the notes on up to 200 first-parent commits back from HEAD, and up to the 20 most recent runs of each command, counting failing runs only when none passed, since a failing run can stop early.

## Timings from other machines

Times from different machines don't compare, but how costly one command is against another mostly holds. So CI's timings say how the commands compare, and this machine's own say how fast it is: the scale is the median, over the commands timed both here and elsewhere, of this machine's time over theirs. A command timed here is estimated at its own median here; one timed only elsewhere, at their median times the scale. With no command timed in both places, timings from elsewhere are shown as they are, and say so.

CI shares its timings by pushing the notes after its runs:

```yaml
- run: jouet test
- run: jouet cost push
```

The push needs permission to write to the repository. It is never forced: when another job pushed first, it fetches their notes, merges them with git's `cat_sort_uniq` strategy and tries again, up to five times. `jouet cost fetch` brings the remote's timings here and merges them with the ones already here. Don't add the notes ref to `git fetch`'s refspecs instead: a fetch replaces a ref with the remote's rather than merging it, which would drop timings taken here and not yet pushed.

This uses the same notes design as [tak](https://tak.jdx.dev).

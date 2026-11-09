---
description: See where your coding-agent sessions spend tokens and time, and which jouet commands would cut it.
---

# jouet audit

`jouet audit` reads the Claude Code and Codex sessions on your machine and reports where they spend input tokens and time. For each costly category it names the jouet command or rule that addresses it, with an estimate of the saving, and it counts habits worth a rule, such as sleep loops.

```text
jouet audit [--since TIME] [--until TIME] [--harness claude|codex|all]
            [--project PATH] [--top N] [--examples] [--what-if SPEC]
            [--jouet] [--json]
```

The flags are listed in the [CLI reference](/cli/audit). `--since` takes a duration ago (`7d`, `12h`), a date (`2026-10-01`) or RFC 3339, and `--until` takes the same forms, so `--since 2026-11-09T06:47:00Z --until 2026-11-09T12:47:00Z` audits one past six-hour window. `--project` keeps sessions that ran in that directory or below it; a relative path is taken from the current directory, with `..` resolved by name rather than by following symlinks. `--top` sets how many categories are listed per harness (default 10; `0` lists all).

## Where it reads

| Harness | Transcripts |
|---|---|
| Claude Code | `$CLAUDE_CONFIG_DIR/projects`, or `~/.claude/projects`: each session and its subagents |
| Codex | `$CODEX_HOME`, or `~/.codex`: `sessions/` and the zstd-compressed `archived_sessions/` |

Files are read in parallel and streamed, so a history of several gigabytes takes about a second once it is in the page cache. Files last written before `--since` are not opened.

## What it counts

A request to the model re-reads the whole context, so a request avoided saves its whole input. `jouet audit` gives each request's input tokens to what the request did: split evenly between the tool calls it issued, by their category, or to "replies" when it called no tool. Claude Code and Codex both record tokens per request; when a source has no token counts, categories are ranked by tool time instead.

When transcripts include a complete, consistent cache split, the harness totals also show cache-read and other input tokens for those requests, alongside the number of requests covered. Other input includes cache writes; it is not necessarily billed as uncached input. Requests with missing, partial or inconsistent cache data are excluded from the split, so its values may cover less than the input total. No split is shown when none is available. The JSON fields are `cache_read_input_tokens`, `other_input_tokens` and `cache_split_requests` under each harness's `totals`. These observations are not a cost estimate or a claim of savings.

The categories are CI polling, process polling, PR threads, PR status, other GitHub, edits, build/test/lint, reads and searches, git, web, agents, replies and other. A shell command is categorized by what it runs, most specific first, so `sleep 30 && gh pr checks 12` is CI polling. jouet's own test, read, search, graph, edit and commit commands count with the corresponding work rather than as other.

The process-polling and CI-polling categories include blocking waits and other checks. Their attributed tokens and time describe where the session spent them; they are not a count of waste or a claim that a longer wait was possible.

Recorded shell-call time sums the durations the transcript attributes to shell calls, not necessarily the full lifetime of their processes. If a command yields a background session, later native wait calls are counted as process polling, not shell time. A shell-based process poll, such as `sleep 10`, counts in both the process-polling category and shell-call time. Unobserved or missing durations are not counted. Tool times can overlap, so their sum is not the task's elapsed wall time. In JSON, these totals are `command_ms` and `tool_ms` respectively.

The savings are estimates and are labelled as such:

- **Process waits:** consecutive short, quiet checks of the same linked job, where the previous check reported it was still running. A longer wait might have avoided later checks. Blocking waits, output, user-facing messages and unlinked calls are not counted as savings. The report also counts linked checks and sequences as observations, and shows cached input separately when available.
- **CI polling:** shown as an observation, with no savings estimate. Adjacent checks can target different runs or already block, and the category does not establish that another request was avoidable.
- **PR threads and PR status:** requests that did the same kind of thing as the request just before them. One `pr threads` or `pr status` call might have replaced each run of them.
- **Edits:** requests whose edit failed and had to be redone.
- **Builds and tests:** no estimate. The report names `jouet test`: `--cmd` keeps a run's whole log out of context and prints its failures, and `--affected` runs only the tests a change affects. Builds cost time more than tokens; mbx shares one build cache between worktrees.

Codex process and cell IDs and Claude Code task IDs link checks within a transcript; the IDs are not printed. A check is short for this estimate only when it explicitly requests at most two seconds and actually returns within that time. Codex checks inside an `exec` cell and Claude checks are observations only: their surrounding work or result does not yet establish the same quiet-check evidence. Shell `sleep` and `pgrep` calls without a job identity are unlinked.

The patterns are `sleep` before a check, test output cut with `| head` or `| tail`, a `GH_TOKEN=` prefix that works around a stale token, scripts or heredocs that rewrite files, GitHub rate-limit messages, `gh` run once per item of a shell loop, and a failed command run again with the same shape right after.

## Fixed context

Claude Code sends the same definitions with every request: each tool's JSON, the skill listing, and the instructions of each connected MCP server. A transcript records them once, when the session starts. For Claude Code, the report adds a "fixed context" section built from each transcript's first snapshot:

- every definition, grouped as something a setting can remove: a built-in tool, an MCP server (its tools and its instructions), a plugin (its skills) or a skill;
- its estimated tokens per request, the transcripts it was in, its tokens times those transcripts' requests, and its calls (a skill's through the `Skill` tool) in every transcript audited;
- for each definition nothing called, the setting that removes it: `permissions.deny` for a built-in tool, `enableArtifact: false` for Artifact, `enabledPlugins` for a plugin, disconnecting an MCP server. The desktop app's own `ccd_*` servers have none that is known;
- the average by week (Monday to Sunday, UTC), naming what grew and what shrank since the week before.

The tokens are estimates from the definitions' sizes: tool JSON at 4.6 characters a token (Claude Code's Artifact tool measured 12.8k tokens for 58k characters), prose at 3.6. A skill run as a slash command may not show as a `Skill` call, so "never called" is an upper bound for skills. Which settings remove a skill's listing line or an MCP server's definitions has not been verified for every Claude Code version; check the next audit after changing one. A week's average mixes sessions and their subagents, which carry fewer definitions.

```console
$ jouet audit --harness claude --since 14d --top 3
…
  fixed context: tool, skill and MCP definitions sent with every request, from each transcript's first snapshot (estimated tokens):
  39.3k per request on average, over 50,675 requests in 595 transcripts: 2.15B tokens, 17.2% of input
  definition              kind  per request  transcripts  tokens  calls
  Artifact                tool        12.8k          566  646.0M      0
  anthropic-skills      plugin         2.8k          529  140.3M      1
  ccd_turn          MCP server         2.6k          522  129.4M  3,272
  … 68 more definitions (--top 0 lists all)

  never called: 43 definitions of up to 29.7k per request; what removes each:
  - Artifact (tool, 12.8k per request, 646.0M tokens): enableArtifact: false
  - Workflow (tool, 2.0k per request, 92.7M tokens): permissions.deny: ["Workflow"]
  - computer-use (MCP server, 1.8k per request, 89.6M tokens): disconnect the MCP server
  … 40 more unused definitions (--top 0 lists all)

  by week (from Monday, UTC):
  2026-10-26  41.3k per request, 264 transcripts
  2026-11-02  40.4k per request, 210 transcripts; shrank: Artifact −237, Workflow −126, ScheduleWakeup −112
  2026-11-09  32.9k per request, 121 transcripts; grew: Bash +129; shrank: anthropic-skills −1.1k, …
```

In JSON, it is `fixed_context` under the harness: `items` (each with `kind`, `name`, `tokens_per_request`, `transcripts`, `requests`, `input_tokens`, `calls` and, when nothing called it, `remove`) and `periods` (each with `start`, `mean_tokens`, `grew` and `shrank`).

## What stays in the context

The categories above charge a request's input to the calls that request made: what the agent did. The "carried" section charges it to what the agent keeps re-reading. Between two requests the context grows by what the first one's calls returned; that growth, times the requests that read it before the next compaction, is charged to those calls, split by the bytes each returned, and to "replies" when the request called no tool. What a stretch between compactions starts with, the fixed context and instructions or a compaction's summary, is shown as the start. A compaction is Claude Code's `compact_boundary` record, or a drop of 20k tokens or more. A context that shrank without one leaves a remainder, `unattributed_tokens` in JSON. With `--examples`, the section also names the files whose reads and edits kept costing most, relative to the session's directory; without it, no file is named.

```console
$ jouet audit --harness claude --since 14d --top 4
…
  carried: each request's input, charged to what put it in the context and how many requests re-read it before a compaction (estimated from context growth):
  start of the session or after a compaction 34.2% (78 compactions)
  category            share  tokens
  reads and searches  26.2%   3.30B
  edits               15.5%   1.96B
  other                5.3%  674.9M
  git                  5.3%  671.7M
```

## What if

`--what-if SPEC` replays the same requests with fewer definitions or a smaller compaction window. `SPEC` is `window=SIZE` (`200k`, `1m`, `200000`), `deny=NAME` with more names after it (`deny=Workflow,Artifact`), or both: `window=200k,deny=Workflow`. A name is one from the fixed-context table: a tool, an MCP server, a plugin or a skill.

- **A denial** takes the definition's estimated tokens off every request of every transcript that carried it. That replays exactly, up to the size estimate, and each denied name is listed with what it saves.
- **A window** is simulated: when the context left after any denials would pass it, the replay compacts to what the transcript started with (before `--since` cut it) plus 19k (the median after real compactions on the machine this was measured on), and charges one read of the whole context for the summary and one cache write of what is left. Where compaction fires inside the window is not known, so the result is a range, from firing at the window to firing 33k below it. A real compaction in the transcript resets the replay. It assumes the same requests follow a compaction, while real sessions re-read some of what was dropped, so the saving is an upper bound.

Each result is shown in tokens and price-weighted, counting an uncached token 1, a cache write 1.25 and a cache read 0.1, with each request keeping its own mix.

```console
$ jouet audit --harness claude --since 14d --what-if window=200k
…
  what if (window 200k): replaying 51,622 requests that read 12.67B tokens (1.41B price-weighted: uncached 1, cache write 1.25, cache read 0.1)
  input 7.17B to 6.60B (-43.5% to -47.9%), price-weighted -36.8% to -37.5%; 453 to 811 compactions added
  the range is compaction firing at the window, then 33k below it; it assumes the same requests after a compaction
```

In JSON it is `what_if` under the harness, with `denied` and one `scenarios` entry per reserve.

## jouet's own costs

`jouet audit --jouet` reports what jouet itself cost instead, per session, for a recurring review such as a scheduled `jouet audit --jouet --since 6h --json`:

- from the hook ledger, as [`jouet stats`](/commands/stats) counts them: stop blocks, and how many a passing check followed before the session next stopped; denials and warnings per rule, and what the agent ran after a denial (a jouet command, something else, or nothing); jouet commands that failed by their own exit code;
- from the transcripts: usage errors (exit 2), crashes, exit codes outside jouet's table, `--json` output that was not JSON, and answers the agent dropped for the raw command right after;
- test logs read within three requests of a `jouet test` run: a `cat`, `grep`, `tail` or Read of its `test-<time>-<pid>-<n>.log`, a sign the summary left out what the agent needed.

A usage error is counted only when it is the whole command, a `cd` before it aside, so the exit code is jouet's: `jouet x --help | head` is not. With `--project`, the ledger counts only for the sessions that ran there, since the ledger does not record where a session ran. Without a ledger (no `jouet setup`), the blocks and denials are zero.

```console
$ jouet audit --jouet --since 6h
jouet audit --jouet: 62 sessions since 2026-11-09T15:42:06Z; jouet cost something in 22
  ledger: 4,062 entries from 2026-11-09T15:42:14Z
  stop blocks: 5 (5 resolved by a passing check before the next stop)
  denials: 33, warnings: 31
    piped-test-output: 12 denied, 22 warned; then a jouet command 8, something else 4, nothing 0
    review-threads: 11 denied, 0 warned; then a jouet command 10, something else 1, nothing 0
    …
  failed jouet runs: 14
  usage errors: 0, crashes: 0, exit codes outside the table: 0, bad --json: 0, answers dropped for the raw command: 0
  test logs read right after `jouet test`: 4 of 145 runs

  by session:
  3be5dbdc claude ~/src/mise/.claude/worktrees/mise-discussion-15502-cc2b47: 1 stop block (1 resolved), piped-test-output denied ×1, piped-test-output warned ×1, review-threads denied ×1, 3 failed jouet runs
    test log read 1 request after `jouet test --cmd -- <v>… && jouet test --cmd -- <v>…`: `grep -v <s> <path> | tail -<n>`
  …
```

Its JSON has `totals`, `rules` and `sessions`, each session with its costs, `rules`, `findings` and `log_reads`. Commands appear only as shapes.

## Privacy

Nothing leaves the machine. The report never shows file contents, prompts or whole commands, only command *shapes*: the program, its subcommands and flags, with every value replaced, so `gh pr checks 15002 --repo jdx/mise` is shown as `gh pr checks <n> --repo <path>`. Heredoc bodies are dropped.

`--examples` adds one example command per shape and pattern. Heredoc bodies are replaced by their line count, the home directory is written `~`, and token-like strings are always redacted: credentials with a known prefix, JSON web tokens, values of variables and flags named as secrets, URL passwords, and long key-like runs.

## Example

An excerpt of a report over about 950 Codex sessions:

```console
$ jouet audit --harness codex --top 5
jouet audit: 947 sessions (read 989 files, 663.0 MB, in 0.4s)

Codex: 947 sessions (965 transcripts with subagents), 216,659 requests, 221,424 tool calls (5,085 failed)
  input 27.50B tokens, output 47.9M; 121,175 commands; recorded shell-call time 119h; tools returned 820.7 MB

  share of input tokens:
  category            share  tokens  requests   calls  time    output
  process polling     37.2%  10.22B    75,517  77,033  281h  216.9 MB
  reads and searches  12.0%   3.29B    27,862  29,072  2.1h  223.7 MB
  CI polling          10.0%   2.74B    21,100  22,189   58h   60.4 MB
  edits                9.0%   2.48B    18,020  18,629   42m    2.9 MB
  git                  8.3%   2.28B    20,198  21,564  4.8h   91.4 MB
  … 8 more categories (--top 0 lists all)

  what would cut it:
  - CI polling (10.0%): jouet wait ci — can wait for checks and show why one failed; use --all to wait for every check (built-in rule `sleep-poll`)
    top shapes:
      gh run view <n> --json <v> --jq <s> (896 calls, 110.1M tokens)
      gh run view <n> --repo <path> --json <v> --jq <s> (621 calls, 71.5M tokens)
      sleep <n>; gh run view <n> --json <v> --jq <s> (607 calls, 91.8M tokens)
      … 5,128 more shapes (--json lists 5)

  patterns:
  - sleep before a check: 7,284 calls in 193 sessions, 1.03B tokens → `jouet wait` blocks until the condition holds, in one call (built-in rule `sleep-poll`)
  - stale-token prefix: 1,529 calls in 51 sessions, 190.8M tokens → fix the token in the environment once, or have the session start without it
```

The excerpt leaves out the edits suggestion, the other patterns and the closing note.

## JSON

`--json` prints one document: `scan` (files and bytes read, files skipped as older than `--since` or outside `--project`, lines that were not JSON, requests left out as copies of ones another transcript already counted, files that could not be read, the time taken) and one entry in `harnesses` per harness, each with `totals`, every category, `suggestions` and `patterns`. A category's `share` is a fraction from 0 to 1 of the harness's input tokens, or of its tool time when `ranked_by` is `duration`. Each category lists its five most frequent shapes and `shape_count`. Fields that are zero or empty are left out.

```console
$ jouet audit --json | jq -c '.harnesses[] | {harness, top: .categories[0].category}'
{"harness":"claude","top":"read_search"}
{"harness":"codex","top":"process_polling"}
```

## Exit codes

`0` with a report, `2` for a usage error, and `3` when no transcripts were found or none matched `--since`, `--until` and `--project` (with `--jouet`, when the ledger has no entries in the window either). `1` with `--jouet` when the ledger cannot be read. A file that cannot be read, or is cut off partway, does not fail the audit: the report lists it, and what came before the error still counts. A directory that cannot be read inside a harness's directory is skipped. A forked or resumed Claude Code session repeats its parent's requests, message IDs and all, and each is counted once.

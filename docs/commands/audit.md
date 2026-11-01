---
description: See where your coding-agent sessions spend tokens and time, and which jouet commands would cut it.
---

# jouet audit

`jouet audit` reads the Claude Code and Codex sessions on your machine and reports where they spend input tokens and time. For each costly category it names the jouet command or rule that addresses it, with an estimate of the saving, and it counts habits worth a rule, such as sleep loops.

```text
jouet audit [--since TIME] [--harness claude|codex|all] [--project PATH]
            [--top N] [--examples] [--json]
```

The flags are listed in the [CLI reference](/cli/audit). `--since` takes a duration ago (`7d`, `12h`), a date (`2026-10-01`) or RFC 3339. `--project` keeps sessions that ran in that directory or below it; a relative path is taken from the current directory, with `..` resolved by name rather than by following symlinks. `--top` sets how many categories are listed per harness (default 10; `0` lists all).

## Where it reads

| Harness | Transcripts |
|---|---|
| Claude Code | `$CLAUDE_CONFIG_DIR/projects`, or `~/.claude/projects`: each session and its subagents |
| Codex | `$CODEX_HOME`, or `~/.codex`: `sessions/` and the zstd-compressed `archived_sessions/` |

Files are read in parallel and streamed, so a history of several gigabytes takes about a second once it is in the page cache. Files last written before `--since` are not opened.

## What it counts

A request to the model re-reads the whole context, so a request avoided saves its whole input. `jouet audit` gives each request's input tokens to what the request did: split evenly between the tool calls it issued, by their category, or to "replies" when it called no tool. Claude Code and Codex both record tokens per request; when a source has no token counts, categories are ranked by tool time instead.

The categories are CI polling, process polling, PR threads, PR status, other GitHub, edits, build/test/lint, reads and searches, git, web, agents, replies and other. A shell command is categorized by what it runs, most specific first, so `sleep 30 && gh pr checks 12` is CI polling.

The process-polling category includes blocking waits and other checks. Its attributed tokens and time describe where the session spent them; they are not a count of waste or a claim that a longer wait was possible.

The savings are estimates and are labelled as such:

- **Process waits:** consecutive short, quiet checks of the same linked job, where the previous check reported it was still running. A longer wait might have avoided later checks. Blocking waits, output, user-facing messages and unlinked calls are not counted as savings. The report also counts linked checks and sequences as observations, and shows cached input separately when available.
- **CI polling, PR threads and PR status:** requests that did the same kind of thing as the request just before them. One `jouet wait`, `pr threads` or `pr status` call might have replaced each run of them.
- **Edits:** requests whose edit failed and had to be redone.
- **Builds and tests** cost time more than tokens; jouet has no command for them, and the report points to mbx.

The patterns are `sleep` before a check, test output cut with `| head` or `| tail`, a `GH_TOKEN=` prefix that works around a stale token, scripts or heredocs that rewrite files, GitHub rate-limit messages, `gh` run once per item of a shell loop, and a failed command run again with the same shape right after.

## Privacy

Nothing leaves the machine. The report never shows file contents, prompts or whole commands, only command *shapes*: the program, its subcommands and flags, with every value replaced, so `gh pr checks 15002 --repo jdx/mise` is shown as `gh pr checks <n> --repo <path>`. Heredoc bodies are dropped.

`--examples` adds one example command per shape and pattern. Heredoc bodies are replaced by their line count, the home directory is written `~`, and token-like strings are always redacted: credentials with a known prefix, JSON web tokens, values of variables and flags named as secrets, URL passwords, and long key-like runs.

## Example

An excerpt of a report over about 950 Codex sessions:

```console
$ jouet audit --harness codex --top 5
jouet audit: 947 sessions (read 989 files, 663.0 MB, in 0.4s)

Codex: 947 sessions (965 transcripts with subagents), 216,659 requests, 221,424 tool calls (5,085 failed)
  input 27.50B tokens, output 47.9M; 121,175 commands ran 119h; tools returned 820.7 MB

  share of input tokens:
  category            share  tokens  requests   calls  time    output
  process polling     37.2%  10.22B    75,517  77,033  281h  216.9 MB
  reads and searches  12.0%   3.29B    27,862  29,072  2.1h  223.7 MB
  CI polling          10.0%   2.74B    21,100  22,189   58h   60.4 MB
  edits                9.0%   2.48B    18,020  18,629   42m    2.9 MB
  git                  8.3%   2.28B    20,198  21,564  4.8h   91.4 MB
  … 8 more categories (--top 0 lists all)

  what would cut it:
  - CI polling (10.0%): jouet wait ci — one call blocks until the checks settle and shows why any failed, instead of a model turn per check (built-in rule `sleep-poll`)
    estimate: 846.4M tokens (3.1%), counting requests that polled CI right after a request that did; a wait would have made each run of polls one call
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

`0` with a report, `2` for a usage error, and `3` when no transcripts were found or none matched `--since` and `--project`. A file that cannot be read, or is cut off partway, does not fail the audit: the report lists it, and what came before the error still counts. A directory that cannot be read inside a harness's directory is skipped. A forked or resumed Claude Code session repeats its parent's requests, message IDs and all, and each is counted once.

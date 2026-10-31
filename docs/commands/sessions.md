---
description: Which coding-agent sessions are working where on this machine, from Claude Code's and Codex's own session files, with warnings for two sessions in one worktree.
---

# jouet sessions

`jouet sessions` shows which agent sessions are working where on this machine: each session's harness, the worktree it last worked in, the branch checked out there, and how long ago it was last active. It reads the harnesses' own session files, so it needs no setup and no hooks.

```text
jouet sessions [--since TIME] [--all]
```

Running several sessions on one repository goes wrong in a few recurring ways: two sessions in one worktree commit each other's uncommitted work, and a session keeps working after its worktree was removed under it. `jouet sessions` warns about both.

```text
$ jouet sessions
*live  claude  fea33135    0s ago  ~/src/jouet/.claude/worktrees/coordination-9bab00  [feat/claim]
 live  claude  e0e4b14d    3s ago  ~/src/jouet/.claude/worktrees/graph-rewrites-29a6db  [claude/graph-rewrites-29a6db]
 live  claude  e97a1961   15s ago  ~/src/jouet/.claude/worktrees/bench-fixes-9e196f  [claude/lean-primer]
 live  claude  b1b2a420    4m ago  ~/src/mr-boxington/.claude/worktrees/target-in-use-a2e83e  [claude/target-in-use-a2e83e]
 live  claude  83321f1e    6m ago  ~/src/jouet/.claude/worktrees/wind-tasks-230fab  [claude/wind-tasks-230fab]
 live  claude  55aa1a11    7m ago  ~/src/jouet/.claude/worktrees/pitchfork-c8b4ad  [claude/pitchfork-c8b4ad]
 live  claude  0860db72   10m ago  ~/src/mise/.claude/worktrees/agentic-toolkit-cli-590ea8  [registry-jouet]
 live  claude  634a3011   12m ago  ~/src/mise/.claude/worktrees/agentic-toolkit-cli-590ea8  [registry-jouet]
(* this session)

warning: 2 live sessions share the work tree ~/src/mise/.claude/worktrees/agentic-toolkit-cli-590ea8 (claude 0860db72, claude 634a3011): each can commit the other's uncommitted work; give one its own worktree

... 66 idle sessions not listed (--all lists them)
```

## What it reads

- **Claude Code:** `~/.claude/projects/*/*.jsonl`, or under `$CLAUDE_CONFIG_DIR`. Subagents' transcripts belong to their session and are not listed.
- **Codex:** `~/.codex/sessions/**/rollout-*.jsonl`, or under `$CODEX_HOME`. Archived sessions are not read.

Only files written within `--since` (default `24h`) are opened, and only a little of each is read:

- the end of a Claude Code transcript, for the latest line with a working directory and branch;
- a Codex rollout's first line (`session_meta`), and its end for the latest `turn_context`.

Nothing a session said is read into the result, and nothing is written.

## What it shows

- **live or idle:** a session is live when its transcript was written in the last 15 minutes. A session waiting on a long command, or on a person, can look idle.
- **the worktree:** the git work tree around the directory the session last worked in. `(gone)` marks a directory that no longer exists.
- **the branch:** the one checked out in that worktree now, read from its `HEAD`. When the worktree is gone, the branch the transcript last recorded.
- **`*`:** the session jouet runs in, from `JOUET_SESSION_ID`, `CLAUDE_CODE_SESSION_ID` or `CODEX_THREAD_ID`.

By default only live sessions are listed, and a line says how many idle ones were left out. `--all` lists the idle ones too.

Paths show the home directory as `~`, and anything that looks like a credential in a path or branch is replaced with `<redacted>`, as `jouet audit` redacts its examples.

## Entire

Where the repository in the current directory has [Entire](https://entire.io) (`entire` on `PATH`, the repository enabled and a working login), `jouet sessions` adds the sessions Entire tracks there, from `entire session list --json`, matched to the ones it found by session ID:

- a session jouet found keeps its own directory and branch, and gets `(Entire: N checkpoints)`;
- a session only Entire knows, such as an agent jouet does not read (Cursor, OpenCode) or a transcript kept elsewhere, is listed from Entire's record, active within `--since`, with `(only in Entire, N checkpoints)`. Entire records the directory the agent started in, which can be the main checkout for a session that works in a worktree.

```text
 live  claude  ac87ea0b   37s ago  ~/src/jouet/.claude/worktrees/secrets-scan-89969a  [claude/secrets-scan]  (Entire: 1 checkpoint)
 live  claude  1246445f    3m ago  ~/src/jouet/.claude/worktrees/db-d424f4  [claude/db-d424f4]  (Entire: 3 checkpoints)
```

Entire is per repository, so only the current repository's sessions are added. A call that fails or times out is a note; `JOUET_ENTIRE=false` turns it off.

## Warnings

- **`shared_worktree`:** two or more live sessions last worked in the same worktree. One of them should move to a worktree of its own.
- **`worktree_gone`:** a live session's directory no longer exists. Some harnesses remove worktrees on their own. An idle session's worktree being gone is usual, since worktrees are cleaned up when the work ends, so that is not a warning.

## JSON

`--json` prints one document:

```json
{
  "sessions": [
    {
      "harness": "claude",
      "id": "0860db72-…",
      "cwd": "~/src/mise/.claude/worktrees/agentic-toolkit-cli-590ea8",
      "worktree": "~/src/mise/.claude/worktrees/agentic-toolkit-cli-590ea8",
      "branch": "registry-jouet",
      "last_active": "2026-10-31T02:25:12Z",
      "idle_secs": 480,
      "live": true
    }
  ],
  "warnings": [
    {
      "code": "shared_worktree",
      "message": "2 live sessions share the work tree …",
      "sessions": ["claude:0860db72-…", "claude:634a3011-…"]
    }
  ],
  "idle_not_listed": 66
}
```

`live`, `current` (the session jouet runs in) and `gone` are present only when true. A session Entire tracks has `entire`: its `status` (`active`, `idle` or `ended`), `checkpoints`, and `only: true` when only Entire knows it. `notes` says why Entire could not be read.

## Exit codes

`0` whether or not there are warnings: it is a listing, like `jouet job ls`. `2` for a `--since` that is not a time.

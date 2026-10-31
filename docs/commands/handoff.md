---
description: Record the facts about a checkout for the next agent session, and check that they still hold when it picks them up.
---

# jouet handoff and pickup

`jouet handoff` records the facts about a checkout for the next session: where the branch stands, what is uncommitted, the PR and what it needs, the last test runs and what is still running. `jouet pickup` prints them in the next session and checks that they still hold. The idea comes from the handoff skills steipete and Matt Pocock published.

```text
jouet handoff [-o FILE] [--no-pr] [--session ID]
jouet pickup [FILE]
```

A handoff holds **facts only**. What the work is, what was tried and what to do next is for the agent to write. The handoff is what that note can rely on.

## What a handoff records

- the worktree path, the branch, its upstream, and how far ahead and behind it is;
- HEAD and its subject, and an operation in progress (a merge, rebase, `am`, cherry-pick, revert or bisect);
- uncommitted files, with their `git status --short` codes, and untracked files;
- the branch's PR (the open one, else the newest) and its [`pr status`](/commands/pr-status) attention codes;
- the session's last test, lint and build runs, as `jouet setup`'s hooks record them;
- background processes running in the worktree, each with a command to attach to it;
- the worktree's [`jouet job`](/commands/job)s, running or ended, each with how to read its log;
- the worktree's [pitchfork](https://pitchfork.jdx.dev) daemons that are running, starting, stopping or failed, from `pitchfork list --project --json`, listed apart from the jobs;
- what could not be gathered, and why: no GitHub remote, no session ID, `--no-pr`.

The last runs come from the session named by `--session`, else `JOUET_SESSION_ID`, else Claude Code's `CLAUDE_CODE_SESSION_ID`. Without them the handoff still works and says so.

Background processes are listed on Linux only, and the list is a heuristic. It includes processes that:

- belong to you and have their working directory inside the worktree;
- are not jouet's own ancestors or siblings (the agent, its shell, the rest of jouet's pipeline);
- are not in use at a terminal: an interactive shell, or whatever runs in a terminal's foreground, such as an editor.

A child whose parent is also listed is folded into it. A process whose stdout goes to a file is attached with `tail -f FILE`; any other with `jouet wait pid PID`. A running job's processes, and a pitchfork daemon's, are listed once, as the job or the daemon.

pitchfork daemons are listed when pitchfork is installed, from the namespace of the worktree's `pitchfork.toml`, which pitchfork derives from the worktree's directory unless the file sets one. Daemons in `global` are left out: that is where pitchfork puts daemons started outside a project, and nothing says which checkout they belong to. Each comes with `pitchfork logs ID -n 50` to read its log, and `jouet wait daemon ID` when it is running.

```text
# Handoff: feat/git-recipes

Written 2026-10-30T18:02:51Z in `/home/me/src/jouet-git`.

- **Branch:** feat/git-recipes, tracking origin/feat/git-recipes (2 ahead, 0 behind)
- **HEAD:** 15b0989e45fc feat(git): add jouet commit
- **PR:** jdx/jouet#20 (open) https://github.com/jdx/jouet/pull/20: feat(git): add git recipes
  - `checks_failing`: 1 failing: lint
- **Last checks:**
  - test passed (exit 0): `mise run test`
- **Uncommitted (1):**
  - ` M` src/handoff.rs
- **Background processes (1):**
  - 543435 `npm run docs:dev`: attach with `tail -f /tmp/docs.log`
```

## Where it goes

`jouet handoff` writes `<git dir>/jouet/handoff.md`. That file belongs to the worktree, sits outside the working tree and is never committed, and `jouet pickup` reads it by default. `-o FILE` writes elsewhere: a `.json` path writes JSON, and `-` prints the handoff to stdout.

The markdown file ends with the same facts as JSON in an HTML comment. A reader sees the markdown, and `pickup` reads exactly what was recorded. The printed text and `pickup` leave the comment out. A list longer than 50 entries is cut, and the cut says how many entries were left out and where to find all of them.

## Picking up

`jouet pickup` prints the handoff, then compares it with the checkout and lists what has drifted:

| Code | Drifted when |
|---|---|
| `worktree` | it is read in another worktree |
| `branch` | another branch is checked out |
| `head` | HEAD has new commits, moved back, or was rewritten |
| `operation` | a merge, rebase or other operation started or finished |
| `upstream`, `behind` | the upstream changed, or moved |
| `uncommitted` | uncommitted files changed state, or their staged or unstaged changes differ |
| `untracked` | untracked files appeared or went away |
| `process_exited` | a recorded process is no longer running |
| `job_ended` | a job that was running has exited, been stopped, or is gone |
| `daemon_stopped`, `daemon_failed` | a pitchfork daemon that was running is stopped, or errored or failed |
| `daemon_gone`, `daemon_restarted` | pitchfork no longer knows a daemon that was running, or it runs with another PID |
| `daemons_unknown` | the handoff recorded daemons, and pitchfork could not be asked about them |

```text
## Drift since 2026-10-30T18:02:51Z (2h ago)

- `head`: 1 new commit since the handoff (15b0989e45fc..4c1d2e3f4a5b)
- `uncommitted`: src/handoff.rs no longer has changes
```

When the handoff recorded jobs or daemons, pickup also lists each one as it is now, under `## Jobs now` and `## pitchfork daemons now`: still running, or how it ended.

pickup exits 0 when nothing has drifted and 1 when something has. It does not fetch the PR again; run [`jouet pr status`](/commands/pr-status) for its current state. A missing handoff is exit 3, and a file that is not a handoff is exit 4.

## JSON

`jouet handoff --json` prints `{"path": ..., "handoff": {...}}`. `jouet pickup --json` prints the path, the handoff, `drift` (left out when empty), `jobs` (each recorded job's `id` and its state `then` and `now`), `daemons` (the same for each daemon, with pitchfork's `error`) and `accurate: true` when nothing has drifted. Fields that are null, false or empty are left out.

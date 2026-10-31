---
description: Advisory claims between agent sessions on one machine, so a session is warned before it edits or commits files another session is working on.
---

# jouet claim

`jouet claim` tells the other agent sessions on this machine which files a session is working on. jouet's hooks then warn a session that edits or commits a path another live session claims, and name that session, its worktree and its branch. A claim is advisory: it never blocks anything.

```text
jouet claim PATH... [--for DUR] [--note TEXT]
jouet claim ls [--all]
jouet claim release [PATH...]
```

It comes from running several sessions on one repository at once. Two sessions that change the same files overwrite each other's edits, and a session that shares a worktree with another commits the other's uncommitted work along with its own. Neither session finds out until the damage is done.

## Before and after

Without claims, a session in another worktree has no way to know that `src/api/` is being reworked, and commits a fix to `src/api/routes.rs` that the rework then conflicts with or overwrites.

With a claim, the first session says what it is doing:

```output
$ jouet claim src/api/ docs/api.md --note "moving the routes to axum" --for 3h
CLAIMED src/api/, docs/api.md for claude session 7f3c2a91, until 2026-10-31T05:39:02Z (3h)
  other sessions are warned when they edit or commit these; `jouet claim release` gives them up
```

A second session, working in another worktree of the same repository, is refused an overlapping claim, and told who holds it:

```output
$ jouet claim src/api/routes.rs
REFUSED other sessions claim overlapping paths; nothing was claimed
  src/api/routes.rs: claimed as src/api/ by claude session 7f3c2a91 in ~/src/app (branch main), 3h left: moving the routes to axum
  next: work elsewhere, or coordinate with that session; its claim lapses when it expires or the session ends
```

If it edits or commits the file anyway, the `claimed-path` rule adds a warning to the call:

```output
Another session is working on this: src/api/routes.rs is claimed as src/api/ by session 7f3c2a91 in /home/me/src/app (branch main), until 2026-10-31T05:39:02Z: moving the routes to axum. Leave it to that session or coordinate first; `jouet claim ls` lists the claims. [jouet rule: claimed-path]
```

## What a claim covers

`PATH` is relative to the current directory and is stored relative to the repository's root, so it means the same in every worktree:

- a file, such as `src/api/routes.rs`;
- a directory, such as `src/api` or `src/api/`, which covers everything under it;
- a glob, such as `docs/*.md` or `src/**/*.rs`: `*` and `?` stay within a directory, and `**` crosses any number of them.

The repository's root itself, and paths outside the repository, are refused (exit 2). Claiming a path again renews it, with the new `--for` and `--note`. A claim call is all or nothing: if any path overlaps another session's claim, nothing is claimed (exit 1).

## Sessions and liveness

A claim belongs to an agent session. The session ID comes from `JOUET_SESSION_ID`, else `CLAUDE_CODE_SESSION_ID` (Claude Code) or `CODEX_THREAD_ID` (Codex), which the harnesses set for the commands they run. The hooks know the same session from their payload's `session_id`, so a session is never warned about its own claims. Outside an agent session, `jouet claim` exits 4.

A claim stops holding when:

- it expires, after `--for` (default 2h);
- its worktree is removed;
- the agent process that made it exits. jouet records `CLAUDE_PID` under Claude Code, or the nearest `codex` process above it under Codex, with its start time, so a reused process ID does not keep a claim alive. Where jouet cannot find the process, the claim lasts until it expires.

`jouet claim` and `jouet claim ls` remove claims that no longer hold. `jouet claim ls --all` lists them first, with why each lapsed. `jouet claim release` gives up the session's own claims on the paths named, or all of them; another session's claim is never released by hand.

## Where claims live

Each claim is a small JSON file under `$XDG_STATE_HOME/jouet/claims/<key>/`, where the key hashes the repository's git common directory. Every worktree of a repository therefore shares one set of claims, and separate clones do not. The hook reads them from the filesystem without starting git, except to list the staged files of a `git commit`.

## The hook rule

The built-in `claimed-path` rule runs before:

- Claude Code's Edit, Write, MultiEdit and NotebookEdit tools (`jouet setup` adds them to the `PreToolUse` matcher; re-run it to pick that up);
- `git commit`: the paths it names, else what is staged, or every changed tracked file with `-a`;
- `jouet commit PATH...` and `jouet edit replace PATH`.

It warns and never denies. Its script asks the `jouet.claims.held(paths)` probe, which [rules](/rules) can use too, and `jouet.git.changed()` for a commit's files.

## JSON

`jouet claim --json` prints `{"claimed": [...]}`, or `{"conflicts": [{"wanted": ..., "claim": {...}}]}` with exit 1. `jouet claim ls --json` prints the claims, each with `mine: true` when it is this session's and, under `--all`, `lapsed` for one that no longer holds. `jouet claim release --json` prints `{"released": [...]}`.

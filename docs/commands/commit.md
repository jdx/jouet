---
description: Commit only the named paths, with the configured trailers and the AI disclosure, never sweeping up the rest of a shared checkout.
---

# jouet commit

`jouet commit` commits exactly the paths you name. Use it where an agent would run `git add -A && git commit`. In a checkout that other sessions share, `add -A` sweeps up whatever else is there: untracked caches, build output, and another session's unfinished work.

```text
jouet commit PATH... -m MSG [-m MSG]... [--trailer 'Key: value']... [--no-trailers] [--dry-run]
jouet commit PATH... -F FILE
```

```sh
$ jouet commit src/commit.rs docs/commit.md -m "feat(git): add jouet commit"
committed 3f2a9c1d0b7e on feat/git-recipes: feat(git): add jouet commit
  added     docs/commit.md  +48 -0
  modified  src/commit.rs   +12 -3
```

The flags are listed in the [CLI reference](/cli/commit).

## What gets committed

- **Only the named paths, as they are in the working tree.** jouet runs `git commit --only`. Anything else that is already staged stays staged and stays out of the commit.
- **Directories work.** A named directory covers its changed files and its untracked files that `.gitignore` does not exclude. Those untracked files are listed as `newly_tracked`.
- **Paths are literal.** `*.rs` names a file called `*.rs`, and pathspec magic such as `:/` has no special meaning.
- **Refused before anything changes:**
  - `-A`, `--all`, a bare `.` and the repository root (exit 2);
  - a path outside the repository (exit 2);
  - a path that does not exist (exit 3);
  - a path with no changes, or one that `.gitignore` excludes (exit 4).

## Hooks

Hooks always run. jouet commit never passes `--no-verify`, and refuses the flag. git's output, hook output included, goes to a log in jouet's log directory. When the commit fails, jouet:

- shows the last 40 lines of that output and the path of the full log;
- exits 1;
- unstages the untracked files it added, so the checkout is left as it was.

```text
commit failed (exit 1); nothing was committed and nothing was left staged
  ... 6 earlier lines
  src/commit.rs:12: trailing whitespace
  full log: /home/me/.local/state/jouet/logs/commit-1793357701000-4242.log
```

A `commit-msg` hook may change the message; the report shows the message as committed.

## Trailers

The message gets trailers through `git interpret-trailers`, which joins an existing trailer block and skips a trailer that is already there. The first source that is set wins:

1. `JOUET_COMMIT_TRAILERS`: one trailer per line. Set but empty means none.
2. `trailers` under `[commit]` in `~/.config/jouet/config.toml` (under `$XDG_CONFIG_HOME` when that is set). An empty list means none.
3. The defaults. Outside an agent there are none. Inside one they are the co-author line its vendor publishes (Claude Code's is `Co-Authored-By: Claude <noreply@anthropic.com>`) and the AI disclosure:

   ```text
   AI-Assisted: Tool: Claude Code; model: unavailable; version: 2.1.309
   ```

`--trailer` adds more, and `--no-trailers` drops all but those.

In any trailer, `{tool}`, `{model}` and `{version}` are filled in from the environment, never guessed:

- The tool and its version come from `AI_AGENT`; for example, `claude-code_2-1-309_agent` is Claude Code 2.1.309. Without `AI_AGENT`, the tool comes from the same agent detection jouet uses for its display mode, and the version is `unavailable`.
- No agent exports the model it runs, so the model comes only from `JOUET_AI_MODEL` and is otherwise `unavailable`.

A config that names the model itself:

```toml
[commit]
trailers = [
  "Co-Authored-By: Claude <noreply@anthropic.com>",
  "AI-Assisted: Tool: {tool}; model: anthropic/claude-opus-5-5; version: {version}",
]
```

## Conventional commits

When the repository asks for conventional commits, jouet checks the subject. It asks when one of these exists:

- a workflow under `.github/workflows/` that mentions conventional commits, commitlint or `action-semantic-pull-request`;
- a commitlint, cocogitto (`cog.toml`) or commitizen config.

The rules are commitlint's `config-conventional` ones that matter for a subject:

- `type(scope)!: description`, where the scope and the `!` are optional;
- a known type;
- a description that does not start with a capitalised word (an acronym such as `CLI` is fine) and does not end with a period;
- at most 100 characters;
- a blank line before the body.

What happens when a subject breaks a rule is set by `conventional` under `[commit]`, or by `JOUET_COMMIT_CONVENTIONAL`:

| Value | Effect |
|---|---|
| `warn` (default) | Commit anyway, with a warning on stderr |
| `refuse` | Refuse with exit 4 |
| `off` | Do not check |

`types` under `[commit]` replaces the accepted types. The default list is `build`, `chore`, `ci`, `docs`, `feat`, `fix`, `perf`, `refactor`, `revert`, `security`, `style` and `test`.

## JSON

`--json` prints one document. Fields that are null, false or empty are left out:

```json
{
  "committed": true,
  "sha": "3f2a9c1d0b7e4c2f8a6d5e1b0c9f8e7d6c5b4a39",
  "branch": "feat/git-recipes",
  "subject": "feat(git): add jouet commit",
  "message": "feat(git): add jouet commit\n\nCo-Authored-By: Claude <noreply@anthropic.com>\nAI-Assisted: Tool: Claude Code; model: unavailable; version: 2.1.309",
  "files": [
    { "path": "docs/commit.md", "status": "added", "added": 48, "removed": 0 },
    { "path": "src/commit.rs", "status": "modified", "added": 12, "removed": 3 }
  ],
  "newly_tracked": ["docs/commit.md"]
}
```

A file's `status` is `added`, `modified`, `deleted` or `type_changed`. `--dry-run` sets `dry_run` instead of `committed` and shows the message as it would be committed. A failed commit has a `failure` with `exit_code`, `output`, `lines_omitted` and `log_path`.

## Exit codes

| Code | Meaning |
|---|---|
| 0 | Committed, or the dry run found something to commit |
| 1 | git refused the commit, usually because a hook failed |
| 2 | Usage: no paths, `-A`, `.`, `--no-verify`, no message or a bad trailer |
| 3 | A named path does not exist |
| 4 | A named path has no changes or is ignored, or `refuse` rejected the subject |

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

## Secrets in fnox projects

In a project that uses [fnox](https://fnox.jdx.dev), jouet asks fnox whether the commit holds the value of one of the project's secrets before it commits. A project uses fnox when there is a `fnox.toml` (or `.fnox.toml`, `.config/fnox.toml`, or a profile or local variant) in the current directory or a directory above it.

jouet builds a temporary git index from HEAD plus the named paths, which is exactly what `git commit --only` will record, and runs `fnox scan --values --staged --format json` against it with `GIT_INDEX_FILE`. fnox resolves the active profile's secrets and looks for their actual values. Other staged work is not scanned, and the real index is never touched.

When fnox finds a value, nothing is committed, the untracked files stay untracked, and jouet exits 1. It names the secret and the file. The value is never printed:

```text
commit refused: fnox found the value of a secret in what would be committed; nothing was committed and nothing was left staged
  STRIPE_KEY  config/app.env:1
  remove the value from those files; read it from the environment, with `fnox exec -- CMD`, instead
```

A clean scan adds a line such as `fnox found no secret values in 2 files`. fnox's warnings about secrets it could not check, such as one with no value set, are passed on with their names.

| Situation | What happens |
|---|---|
| fnox is not on `PATH` | The commit goes ahead with a warning that it was not scanned (`secret_scan.status` is `skipped`). |
| fnox fails, for example because it cannot decrypt the secrets | Refused with exit 4 and fnox's error. A security check that could not run does not pass. |
| fnox fails inside `fnox proxy run` | The commit goes ahead with a warning. The agent only ever had placeholders, and fnox usually cannot resolve the real values there. |
| `--dry-run` | Not scanned. |

Turn the scan off with `secret_scan = false` under `[commit]` in `~/.config/jouet/config.toml`, or `JOUET_COMMIT_SECRET_SCAN=off` for one run. It is on by default.

The scan has the limits of `fnox scan --values`. It finds exact values only, so an encoded, split or partial value gets past it, and so does a secret shorter than 8 characters, which fnox skips. Agents running jouet's hooks are also kept from printing secrets in the first place. See [the fnox rules](/setup#fnox).

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

In a fnox project, `secret_scan` has the `tool` (`fnox`), a `status` (`clean`, `found` or `skipped`), `files_scanned`, and, when values were found, `findings`, each with the `secret`, `path` and `line`. A skipped scan has a `reason`. fnox's warnings are in `notes`.

## Exit codes

| Code | Meaning |
|---|---|
| 0 | Committed, or the dry run found something to commit |
| 1 | git refused the commit, usually because a hook failed, or fnox found a secret's value in it |
| 2 | Usage: no paths, `-A`, `.`, `--no-verify`, no message or a bad trailer |
| 3 | A named path does not exist |
| 4 | A named path has no changes or is ignored, `refuse` rejected the subject, or fnox could not scan the commit |

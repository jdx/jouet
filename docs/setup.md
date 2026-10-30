---
description: Install jouet's hooks and skill into Claude Code or Codex, so agents are steered toward jouet's commands and cannot finish on a failing check.
---

# Agent setup and hooks

`jouet setup` connects jouet to a coding agent's harness. It installs two things:

- **A skill** that tells the agent when to reach for jouet. It is deliberately thin: the flags live in the binary, behind `jouet agent-help`, so the skill never goes stale when jouet is upgraded.
- **Hooks** that call back into `jouet hook`. They prime each session with a short guide, run jouet's [rules](/rules), which deny a handful of costly hand-rolled patterns and name the jouet command or git habit to use instead, and stop the agent from finishing while a test, lint or build run failed, or after only part of the test suite ran on changed code.

```sh
jouet setup claude              # Claude Code: ~/.claude/settings.json and ~/.claude/skills/jouet/
jouet setup codex               # Codex: ~/.codex/hooks.json and ~/.agents/skills/jouet/
jouet setup claude --project    # this repository, for you alone (.claude/settings.local.json)
jouet setup claude --shared     # this repository, for everyone (.claude/settings.json)
jouet setup codex --dry-run     # show the diff, write nothing
jouet setup codex --uninstall   # remove what jouet added
```

::: warning Early development
The hooks, their rules and the settings below are new and may change between releases.
:::

## What setup changes

| Harness | Scope | Hooks | Skill |
|---|---|---|---|
| Claude Code | `--global` (default) | `~/.claude/settings.json` (`$CLAUDE_CONFIG_DIR` if set) | `~/.claude/skills/jouet/SKILL.md` |
| Claude Code | `--project` | `.claude/settings.local.json` | `.claude/skills/jouet/SKILL.md` |
| Claude Code | `--shared` | `.claude/settings.json` | `.claude/skills/jouet/SKILL.md` |
| Codex | `--global` (default) | `~/.codex/hooks.json` (`$CODEX_HOME` if set) | `~/.agents/skills/jouet/SKILL.md` |
| Codex | `--shared` | `.codex/hooks.json` | `.agents/skills/jouet/SKILL.md` |

- **`--project` is yours alone.** It writes at the root of the enclosing git repository, into Claude Code's `.claude/settings.local.json`, which Claude Code reads alongside `settings.json` but which is not meant to be committed. Claude Code ignores that file in git once it has written to it itself; until then, and for the skill, setup says when git does not ignore them, so you can list them in `.gitignore` or `.git/info/exclude`.
- **`--shared` is for everyone who clones the repository.** It writes the tracked `.claude/settings.json` or `.codex/hooks.json`, for a team that has agreed to use jouet. Since the hooks do nothing where jouet is not installed, contributors without it are not broken, but they get no hooks either.
- **Codex has no hooks file of your own in a project.** It reads project hooks only from the repository's `.codex/hooks.json` and `.codex/config.toml`, so `jouet setup codex --project` is refused: use `--global`, or `--shared` to install there for everyone.
- **Merges, never clobbers.** jouet adds its own matcher group to each event and leaves every other hook, setting and key order as it was. A file that is not valid JSON, or whose `hooks` is not an object of arrays, is refused (exit 4) and left alone.
- **Re-running is a no-op.** jouet recognises its entries by their command, which runs `jouet hook ...`, and only rewrites them when they differ from what this version installs. Entries an older version installed, such as the bare `jouet hook claude stop`, are replaced in place, not duplicated.
- **`--uninstall` removes only jouet's entries**, then any group or event that removing them left empty. At project scope it removes them from both `settings.local.json` and `settings.json`, whichever setup wrote. A settings file left empty stays, as `{}`.
- **The skill is jouet's only while it is unedited.** setup ends the `SKILL.md` it writes with a line holding a hash of the rest. It replaces or removes that file only while the hash still matches, and keeps any other `skills/jouet/SKILL.md`, including one you have edited, with a note.
- **Codex asks before running new hooks.** Codex skips new or changed hooks until they are trusted: after `jouet setup codex`, run `/hooks` in Codex and trust jouet's. Upgrading from a version that installed bare commands changes them, so they need trusting again.

### When jouet is not on PATH

Each hook's command checks for jouet first and exits 0 silently when it is not on `PATH`, the way the Entire CLI's hooks do:

```sh
sh -c 'command -v jouet >/dev/null 2>&1 || exit 0; exec jouet hook claude pre-tool-use'
```

So a contributor without jouet, or an app started with a shorter `PATH` than your shell's, gets no hooks rather than an error on every tool call. setup says so when jouet is not on `PATH` where you run it. The command is `sh -c` because Codex runs hooks with your login shell, which may not be a POSIX one. On Windows, Claude Code runs hooks with Git Bash, which has `sh`; without Git Bash it falls back to PowerShell, where this command fails, as a bare `jouet` would when it is missing. Codex runs them with PowerShell, so its handlers also get a `commandWindows`: `if (Get-Command jouet -ErrorAction Ignore) { jouet hook codex pre-tool-use }`.

The hooks find jouet on `PATH` rather than at the path of the jouet that ran setup. That path changes on every upgrade with mise, Homebrew and other version managers, differs between machines that share a settings file, and means nothing to anyone else in a `--shared` install. A hook pointing at a path that is gone would skip just as silently, but on every machine after every upgrade.

### Worktrees

Claude Code reads `.claude/settings.local.json` from the main checkout's root, also in sessions that run in a linked worktree such as the desktop app's `.claude/worktrees/*`, so `--project` writes it there even when you run it from a worktree. The skill is read through from the main checkout only when the worktree has no `.claude/skills` directory of its own; a repository that tracks `.claude/skills` has one in every worktree. On Windows Claude Code reads the local file from each worktree instead, and a new worktree starts without it unless the repository's [`.worktreeinclude`](https://code.claude.com/docs/en/worktrees#copy-gitignored-files-into-worktrees) lists it.

If you work across many repositories or worktrees, install with `--global` instead, and choose which hooks run with the [settings](#settings) below.

## The hooks

| Event | Claude Code | Codex | What jouet does |
|---|---|---|---|
| Session start | `SessionStart` | `SessionStart` | Adds a few lines of context: the installed commands and the active rules. |
| Before a shell command | `PreToolUse` (`Bash`) | `PreToolUse` (`^Bash$`) | Runs the pre-tool-use rules: denies the command with one line of advice, or passes on a warning. |
| After a shell command | `PostToolUse`, `PostToolUseFailure` (`Bash`) | none | Runs the post-tool-use rules, which record test, lint and build runs. |
| Stop | `Stop` | `Stop` | Blocks finishing once while a recorded check failed, or when only partial test runs followed a change. |

Every hook reads the harness's JSON on stdin and **fails open**: input it cannot read, a broken config file, a bad `JOUET_DISPLAY`, a rule that throws or runs out of its budget, or a crash all exit 0 with no decision, so a problem in jouet never blocks the agent. Rule errors are reported on stderr. A pre-tool-use hook takes about 2 ms in a release build, starting the process included.

### Steering rules

The rules are Luau scripts: built in, yours in `~/.config/jouet/rules`, and a repository's `.jouet/rules` once you trust it. Yours only warn unless their header says `mode: deny`, and no rule of yours can replace or overrule a built-in one. [Rules](/rules) covers writing, testing and replacing them; `jouet rule list` shows which run. A rule only advises. jouet denies the command or warns about it and tells the agent what to do instead; it never rewrites a command. The agent sees the advice and the rule's name, for example:

```text
Stage explicit paths (`git add path/to/file ...`): `git add -A` and `git add .` sweep in caches, scratch files and other sessions' work. [jouet rule: git-add-all]
```

The built-in rules:

| Rule | Denies | Suggests |
|---|---|---|
| `sleep-poll` | `sleep N` together with `gh pr checks`, `gh run view` or a `statusCheckRollup` query | `jouet wait ci` |
| `review-threads` | `gh api graphql` (or `curl` to the GraphQL API) with a `reviewThreads` query | `jouet pr threads` |
| `python-edit` | a `python` heredoc or `-c` script that calls `.replace(` and writes a file | `jouet edit` |
| `git-add-all` | `git add -A`, `--all`, `.` or `:/` | staging explicit paths |
| `git-stash` | a bare `git stash`, `push` or `save` without a message, `pop`, `apply` or `drop` without a stash named, and `clear` | `git stash push -m <tag>` and `git stash apply <sha>`, or a WIP commit, because every worktree of a repository shares one stash |
| `no-verify` | `--no-verify` on any git command, and `git commit -n` | fixing what the hook reports |
| `force-push` | `git push --force`, `-f` or a `+refspec` without `--force-with-lease` | `--force-with-lease` |
| `push-untested` | warns (does not deny) on `git push` and `gh pr create` when no full test run has passed at the current tree | running the full suite first, or saying why it is not needed |

A rule that names a jouet command is only active when the installed jouet has that command, so the hooks never suggest something that is not there.

The rules read the command the way a shell does: quoted strings, here-document bodies and comments are data, so `git commit -m "never git add -A"` passes, while commands inside `$(...)` and `sh -c '...'` are checked. Wrappers such as `env`, `sudo`, `timeout`, `rtk` and `mise x ... --` are looked through.

### Finishing on a failing check

The stop hook keeps an agent from declaring success while a test, lint or build run it made failed, or after it only ran part of the test suite on changed code, in the spirit of "verification before completion". It is conservative by design:

- **What counts as a check.** Whatever the post-tool-use rules record. The built-in `checks` rule records `cargo test`, `build`, `check`, `clippy` and `fmt --check` (also through `mbx`), `mise run` tasks named `test*`, `lint*`, `check*`, `build*` or `ci`, `npm`/`pnpm`/`yarn`/`bun` test and scripts with those names, `make`/`just` targets, `go test`/`vet`/`build`, `pytest`, `vitest`, `jest`, `bats`, `hk check` and a few common linters; replace it with a `checks.luau` of your own to change that.
- **Full or partial.** Each run is recorded with its scope: `full` when it covered the whole project, or `partial` with what it selected. For cargo, `-p` (unless the packages named are every workspace member), `--test`, `--lib`, `--bin`, `--exclude`, a test-name filter, `--exact` or `--skip` after `--`, and nextest's `-E` make a run partial; `--workspace`, a plain run in a single-package project, and a plain run at a virtual manifest without `default-members` are full. A plain `cargo test` at the root of a workspace that is also a package tests only that package, so it is partial. A task runner's task is full when it is run alone by a plain name (`mise run test`, `npm test`, `make check`), and partial with arguments or a sub-task (`mise run test:e2e edit`). `go test ./...`, and `pytest`, `vitest` or `jest` without paths or name filters, are full. Anything else, including a `cd` the rule cannot follow, is partial.
- **When a failure counts.** Only when the check is the command's last pipeline, so its exit status is the command's: `cd crate && cargo test` counts, `cargo test; echo done` does not.
- **What clears a failure.** A later full pass of the same kind, or a later pass of a partial run that selected the same thing (`cargo test -p a` failing, then `cargo test -p a` passing). A partial pass never clears a full failure: `cargo test -p a` passing after `cargo test --workspace` failed still blocks, and says only a full run clears it.
- **Partial runs after a change.** When the newest test run is a partial pass and the tree has changed since the last full test pass (or there has been none in the session), the hook blocks once and asks for the full suite, or for a reason it is not needed. Partial runs at the tree the full suite passed at are fine. Lint and build runs are not asked about.
- **The tree.** Each run records the tree it ran against: a hash of the content of every tracked file, staged or not, built from `git ls-files -s` and `git hash-object` (without `-w`) for files changed in the work tree, so nothing is written to the repository. Untracked and ignored files are left out, and a commit with the same content hashes the same, so committing what was tested keeps the tree. (`git stash create` gives a real tree id, but writes objects on every check.)
- **Blocking once.** The hook blocks with a one-line reason, and if the agent stops again straight away (the harness reports `stop_hook_active`), its answer stands. A check that cannot pass never traps the agent.
- **Where runs are recorded.** Claude Code reports each command's exit code to the post-tool-use hooks, which keep each kind's newest run, newest full pass and uncleared failures in `$XDG_STATE_HOME/jouet/sessions/<session>.json`. Updates take a lock, so commands finishing at once cannot drop each other's runs, and files untouched for a week are removed. Codex sends hooks no exit code, so its stop hook reads the commands in the last 8 MiB of the session transcript instead, and runs them through the same rules. That format is Codex's own: if it changes, the hook stops blocking rather than blocking wrongly. The transcript does not say which tree each command saw, so for Codex any partial test run after a full one asks for the full suite once.

### Before pushing and landing

The same records guard what leaves the machine:

- **`git push` and `gh pr create`.** The `push-untested` rule warns when no full test run has passed at the current tree: none is recorded, the last one failed, or the tree has changed since it passed. It stays quiet in a work tree where the hooks have never recorded a check, and on `git push --dry-run` and deletions.
- **`jouet pr land`** refuses (exit `4`) unless the ledger has a full test run that passed at the tree of the pull request's head. `--skip-verify REASON` lands it anyway, and the reason is kept in the ledger. See [`jouet pr land`](/commands/pr-land#tests-before-landing).

Known false positives and misses:

- A check you expected to fail, such as a test written to fail first or a failure that is already on the main branch, still blocks once. Say so in the reply and stop again.
- A full pass of a different selection does not clear a partial failure unless it is a full run: `cargo test -p a` failing, then `cargo test -p a -p b` passing, still blocks once.
- A check piped through `head` or `tail` reads as whatever that command returned, so its failures are missed unless `set -o pipefail` is on.
- A command that changes tracked files and then runs the tests, such as a formatter in the same script, is recorded at the tree after it finished.
- Runs in the background, and checks run through a script the hook cannot see into, are not recorded.

### The ledger

Every hook appends one compact JSON line to `$XDG_STATE_HOME/jouet/ledger/ledger.jsonl`. It holds the harness, the session id, a 16-character hash identifying the work tree (never its path), the time, the tool, the command's *shape*, the exit code and duration where the post-tool-use hooks report them, and each rule's decision (`deny`, `warn`, `record:<kind>`, or the stop check's `block`). A recorded check also carries its `scope` (`full` or `partial`, never what it selected) and the `tree` it ran against. Lines written before jouet kept these read as partial, with no tree. `jouet pr land --skip-verify` adds a `skip-verify` line with the reason given, credentials redacted. [Self-winding](/winding) measures each rule from it, and drafts new rules from the patterns in it.

```json
{"ts":1793380000,"harness":"claude","session":"s1","event":"pre-tool-use","tool":"Bash","shape":"git stash","decisions":[{"rule":"git-stash","action":"deny"}]}
{"ts":1793380090,"harness":"claude","session":"s1","event":"post-tool-use","tool":"Bash","shape":"cargo test -p <arg>","exit_code":101,"duration_ms":41200,"tree":"5d0c3f2a91b7e64c08f1a2b3c4d5e6f7","decisions":[{"rule":"checks","action":"record:test","scope":"partial"}]}
```

- **A shape never holds values.** It keeps each command's program, up to two of that program's known subcommands (`git stash push`, `gh pr checks`) and its flag names. Every other positional (paths, task and script names, `echo`'s words), flag values, message text, here-documents and environment assignments become placeholders, and anything token-like is redacted. `GH_TOKEN=... git -C /repo commit -m "fix it"` is recorded as `git -C <arg> commit -m <arg>`. A loop, conditional, `case` or subshell keeps its structure, with its list, patterns and names as placeholders: `for pr in 1 2; do gh pr checks $pr; done` is `for <arg> in <arg>…; do gh pr checks <arg>; done` (see [compound commands](/winding#compound-commands)).
- **Overrides.** When a command is denied or warned about and the session runs a command of the same shape within ten minutes, and it goes ahead this time, the ledger records an `override` of that rule. It is a heuristic for measuring rules later: same shape means the same program, subcommands and flags, whatever the values. Each denial or warning is counted once.
- **Rotation.** The file is rotated at 4 MiB, and rotated files are removed after 30 days.
- **Turning it off.** `[hooks] ledger = false` or `JOUET_HOOKS_LEDGER=0`.

Explicit `allow` decisions are not recorded: under the [floor](/rules#where-rules-come-from) an allow decides nothing. The stop check still keeps its own small state file.

## Settings

Every hook, every rule and the [`jouet stats`](/commands/stats) summary line can be turned off in `~/.config/jouet/config.toml` (`$XDG_CONFIG_HOME/jouet/config.toml`) or with an environment variable:

```toml
[hooks]
enabled = true # JOUET_HOOKS: every hook
steer = true   # JOUET_HOOKS_STEER: the pre-tool-use rules
prime = true   # JOUET_HOOKS_PRIME: the session-start guide
verify = true  # JOUET_HOOKS_VERIFY: the stop check
ledger = true  # JOUET_HOOKS_LEDGER: the ledger

[rules]
disable = ["git-stash", "no-verify"] # JOUET_RULES_DISABLE=git-stash,no-verify; "all" for every rule

[stats]
summary = "quips" # JOUET_STATS_SUMMARY: quips, plain, or off for no line

[wind]
enabled = true # JOUET_WIND: self-winding, drafting rules from the ledger
```

[Self-winding](/winding) is on after setup: the session-start primer shows at most two notes about rules jouet drafted from the ledger. `JOUET_WIND=0` turns it off.

## The skill and `agent-help`

`jouet skill` prints the skill `jouet setup` installs, about 25 lines of when and why that end in "run `jouet agent-help`". `--format agents-md` prints the same text as a snippet for an `AGENTS.md`, for harnesses without skills.

`jouet agent-help` prints a guide for the whole tool in under 80 lines, and `jouet agent-help COMMAND` one command in full: when to reach for each command, its flags, the output conventions and the exit-code table. Both are generated from the binary's own CLI definition, so they describe exactly the installed version. When [mbx](https://github.com/jdx/mr-boxington), rtk or beads (`bd`) is on `PATH`, the guide says what each is for instead of jouet duplicating it.

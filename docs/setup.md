---
description: Install jouet's hooks and skill into Claude Code or Codex, so agents are steered toward jouet's commands and cannot finish on a failing check.
---

# Agent setup and hooks

`jouet setup` connects jouet to a coding agent's harness. It installs two things:

- **A skill** that tells the agent when to reach for jouet. It is deliberately thin: the flags live in the binary, behind `jouet agent-help`, so the skill never goes stale when jouet is upgraded.
- **Hooks** that call back into `jouet hook`. They prime each session with a short guide, run jouet's [rules](/rules), which deny a handful of costly hand-rolled patterns and name the jouet command or git habit to use instead, and stop the agent from finishing while its last test, lint or build run failed.

```sh
jouet setup claude              # Claude Code: ~/.claude/settings.json and ~/.claude/skills/jouet/
jouet setup codex               # Codex: ~/.codex/hooks.json and ~/.agents/skills/jouet/
jouet setup claude --project    # this repository's .claude/ instead
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
| Claude Code | `--project` | `.claude/settings.json` | `.claude/skills/jouet/SKILL.md` |
| Codex | `--global` (default) | `~/.codex/hooks.json` (`$CODEX_HOME` if set) | `~/.agents/skills/jouet/SKILL.md` |
| Codex | `--project` | `.codex/hooks.json` | `.agents/skills/jouet/SKILL.md` |

`--project` writes at the root of the enclosing git repository. A project file is shared with everyone who clones it, and a hook whose command is not installed fails on every tool call, so use `--project` only where everyone has jouet.

- **Merges, never clobbers.** jouet adds its own matcher group to each event and leaves every other hook, setting and key order as it was. A file that is not valid JSON, or whose `hooks` is not an object of arrays, is refused (exit 4) and left alone.
- **Re-running is a no-op.** jouet recognises its entries by their command, `jouet hook ...`, and only rewrites them when they differ from what this version installs.
- **`--uninstall` removes only jouet's entries**, then any group or event that removing them left empty. A settings file left empty stays, as `{}`.
- **The skill is jouet's only while it is unedited.** setup ends the `SKILL.md` it writes with a line holding a hash of the rest. It replaces or removes that file only while the hash still matches, and keeps any other `skills/jouet/SKILL.md`, including one you have edited, with a note.
- **Codex asks before running new hooks.** Codex skips new or changed hooks until they are trusted: after `jouet setup codex`, run `/hooks` in Codex and trust jouet's.

## The hooks

| Event | Claude Code | Codex | What jouet does |
|---|---|---|---|
| Session start | `SessionStart` | `SessionStart` | Adds a few lines of context: the installed commands and the active rules. |
| Before a shell command | `PreToolUse` (`Bash`) | `PreToolUse` (`^Bash$`) | Runs the pre-tool-use rules: denies the command with one line of advice, or passes on a warning. |
| After a shell command | `PostToolUse`, `PostToolUseFailure` (`Bash`) | none | Runs the post-tool-use rules, which record test, lint and build runs. |
| Stop | `Stop` | `Stop` | Blocks finishing once while the last recorded check failed. |

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

A rule that names a jouet command is only active when the installed jouet has that command, so the hooks never suggest something that is not there.

The rules read the command the way a shell does: quoted strings, here-document bodies and comments are data, so `git commit -m "never git add -A"` passes, while commands inside `$(...)` and `sh -c '...'` are checked. Wrappers such as `env`, `sudo`, `timeout`, `rtk` and `mise x ... --` are looked through.

### Finishing on a failing check

The stop hook keeps an agent from declaring success while the last test, lint or build run it made failed, in the spirit of "verification before completion". It is conservative by design:

- **What counts as a check.** Whatever the post-tool-use rules record. The built-in `checks` rule records `cargo test`, `build`, `check`, `clippy` and `fmt --check` (also through `mbx`), `mise run` tasks named `test*`, `lint*`, `check*`, `build*` or `ci`, `npm`/`pnpm`/`yarn`/`bun` test and scripts with those names, `make`/`just` targets, `go test`/`vet`/`build`, `pytest`, `vitest`, `jest`, `bats`, `hk check` and a few common linters; replace it with a `checks.luau` of your own to change that.
- **When a failure counts.** Only when the check is the command's last pipeline, so its exit status is the command's: `cd crate && cargo test` counts, `cargo test; echo done` does not. Each kind keeps only its latest run, so a later passing `cargo test` clears an earlier failure.
- **Blocking once.** The hook blocks with a one-line reason, and if the agent stops again straight away (the harness reports `stop_hook_active`), its answer stands. A check that cannot pass never traps the agent.
- **Where runs are recorded.** Claude Code reports each command's exit code to the post-tool-use hooks, which keep the latest run of each kind in `$XDG_STATE_HOME/jouet/sessions/<session>.json`. Updates take a lock, so commands finishing at once cannot drop each other's runs, and files untouched for a week are removed. Codex sends hooks no exit code, so its stop hook reads the commands in the last 8 MiB of the session transcript instead, and runs them through the same rules. That format is Codex's own: if it changes, the hook stops blocking rather than blocking wrongly.

Known false positives and misses:

- A check you expected to fail, such as a test written to fail first or a failure that is already on the main branch, still blocks once. Say so in the reply and stop again.
- A narrower passing run clears a broader failure: `cargo test -p a` passing after `cargo test` failed counts as passing.
- A check piped through `head` or `tail` reads as whatever that command returned, so its failures are missed unless `set -o pipefail` is on.
- Runs in the background, and checks run through a script the hook cannot see into, are not recorded.

### The ledger

Every hook appends one compact JSON line to `$XDG_STATE_HOME/jouet/ledger/ledger.jsonl`. It holds the harness, the session id, the time, the tool, the command's *shape*, the exit code and duration where the post-tool-use hooks report them, and each rule's decision (`deny`, `warn`, `record:<kind>`, or the stop check's `block`). jouet will use the ledger to measure how useful each rule is, and later to let agents learn new rules from it.

```json
{"ts":1793380000,"harness":"claude","session":"s1","event":"pre-tool-use","tool":"Bash","shape":"git stash","decisions":[{"rule":"git-stash","action":"deny"}]}
{"ts":1793380090,"harness":"claude","session":"s1","event":"post-tool-use","tool":"Bash","shape":"cargo test -p <arg>","exit_code":101,"duration_ms":41200,"decisions":[{"rule":"checks","action":"record:test"}]}
```

- **A shape never holds values.** It keeps each command's program, up to two of that program's known subcommands (`git stash push`, `gh pr checks`) and its flag names. Every other positional (paths, task and script names, `echo`'s words), flag values, message text, here-documents and environment assignments become placeholders, and anything token-like is redacted. `GH_TOKEN=... git -C /repo commit -m "fix it"` is recorded as `git -C <arg> commit -m <arg>`.
- **Overrides.** When a command is denied or warned about and the session runs a command of the same shape within ten minutes, and it goes ahead this time, the ledger records an `override` of that rule. It is a heuristic for measuring rules later: same shape means the same program, subcommands and flags, whatever the values. Each denial or warning is counted once.
- **Rotation.** The file is rotated at 4 MiB, and rotated files are removed after 30 days.
- **Turning it off.** `[hooks] ledger = false` or `JOUET_HOOKS_LEDGER=0`.

Explicit `allow` decisions are not recorded: under the [floor](/rules#where-rules-come-from) an allow decides nothing. The stop check still keeps its own small state file.

## Settings

Every hook, and every rule, can be turned off in `~/.config/jouet/config.toml` (`$XDG_CONFIG_HOME/jouet/config.toml`) or with an environment variable:

```toml
[hooks]
enabled = true # JOUET_HOOKS: every hook
steer = true   # JOUET_HOOKS_STEER: the pre-tool-use rules
prime = true   # JOUET_HOOKS_PRIME: the session-start guide
verify = true  # JOUET_HOOKS_VERIFY: the stop check
ledger = true  # JOUET_HOOKS_LEDGER: the ledger

[rules]
disable = ["git-stash", "no-verify"] # JOUET_RULES_DISABLE=git-stash,no-verify; "all" for every rule
```

## The skill and `agent-help`

`jouet skill` prints the skill `jouet setup` installs, about 20 lines of when and why that end in "run `jouet agent-help`". `--format agents-md` prints the same text as a snippet for an `AGENTS.md`, for harnesses without skills.

`jouet agent-help` prints a guide for the whole tool in under 60 lines, and `jouet agent-help COMMAND` one command in full: when to reach for each command, its flags, the output conventions and the exit-code table. Both are generated from the binary's own CLI definition, so they describe exactly the installed version. When [mbx](https://github.com/jdx/mr-boxington), rtk or beads (`bd`) is on `PATH`, the guide says what each is for instead of jouet duplicating it.

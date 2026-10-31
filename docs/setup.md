---
description: Install jouet's hooks into Claude Code, Codex, Cursor, Gemini CLI or Copilot, so agents are steered toward jouet's commands and cannot finish on a failing check.
---

# Agent setup and hooks

`jouet setup` connects jouet to a coding agent's harness. It installs **hooks** that call back into `jouet hook`. They prime each session with a few lines about jouet, run jouet's [rules](/rules), which deny a handful of costly hand-rolled patterns and name the jouet command or git habit to use instead, and stop the agent from finishing while a test, lint or build run failed, or after only part of the test suite ran on changed code.

The [skill](#the-skill-and-agent-help) is installed only with `--skill`, or when priming is turned off: the primer already tells the agent what the skill would, in every session.

```sh
jouet setup claude              # Claude Code: ~/.claude/settings.json
jouet setup codex               # Codex: ~/.codex/hooks.json
jouet setup cursor              # Cursor: ~/.cursor/hooks.json
jouet setup gemini              # Gemini CLI: ~/.gemini/settings.json
jouet setup copilot             # Copilot CLI: ~/.copilot/hooks/jouet.json
jouet setup claude --skill      # also ~/.claude/skills/jouet/SKILL.md
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
| Cursor | `--global` (default) | `~/.cursor/hooks.json` | `~/.agents/skills/jouet/SKILL.md` |
| Cursor | `--shared` | `.cursor/hooks.json` | `.agents/skills/jouet/SKILL.md` |
| Gemini CLI | `--global` (default) | `~/.gemini/settings.json` | `~/.agents/skills/jouet/SKILL.md` |
| Gemini CLI | `--shared` | `.gemini/settings.json` | `.agents/skills/jouet/SKILL.md` |
| Copilot | `--global` (default) | `~/.copilot/hooks/jouet.json` (`$COPILOT_HOME/hooks` if set) | `~/.agents/skills/jouet/SKILL.md` |
| Copilot | `--shared` | `.github/hooks/jouet.json` | `.agents/skills/jouet/SKILL.md` |

- **`--project` is yours alone.** It writes at the root of the enclosing git repository, into Claude Code's `.claude/settings.local.json`, which Claude Code reads alongside `settings.json` but which is not meant to be committed. Claude Code ignores that file in git once it has written to it itself; until then, and for the skill, setup says when git does not ignore them, so you can list them in `.gitignore` or `.git/info/exclude`.
- **`--shared` is for everyone who clones the repository.** It writes the tracked `.claude/settings.json` or `.codex/hooks.json`, for a team that has agreed to use jouet. Since the hooks do nothing where jouet is not installed, contributors without it are not broken, but they get no hooks either.
- **Only Claude Code has a project hooks file of your own.** Codex reads project hooks only from the repository's `.codex/hooks.json` and `.codex/config.toml`, Cursor from `.cursor/hooks.json`, Gemini CLI from `.gemini/settings.json` and Copilot from `.github/hooks/*.json`, so `--project` is refused for them: use `--global`, or `--shared` to install there for everyone.
- **Merges, never clobbers.** jouet adds its own matcher group (in Cursor's and Copilot's files, its own handler) to each event and leaves every other hook, setting and key order as it was. A file that is not valid JSON, or whose `hooks` is not an object of arrays, is refused (exit 4) and left alone.
- **Re-running is a no-op.** jouet recognises its entries by their command, which runs `jouet hook ...`, and only rewrites them when they differ from what this version installs. Entries an older version installed, such as the bare `jouet hook claude stop`, are replaced in place, not duplicated.
- **`--uninstall` removes only jouet's entries**, then any group or event that removing them left empty. At project scope it removes them from both `settings.local.json` and `settings.json`, whichever setup wrote. A settings file left empty stays, as `{}`; Copilot's `jouet.json`, which is jouet's alone, is removed.
- **The skill is jouet's only while it is unedited.** setup ends the `SKILL.md` it writes with a line holding a hash of the rest. It replaces or removes that file only while the hash still matches, and keeps any other `skills/jouet/SKILL.md`, including one you have edited. Running setup without `--skill` removes the skill an earlier version installed, while it is unedited.
- **Codex asks before running new hooks.** Codex skips new or changed hooks until they are trusted: after `jouet setup codex`, run `/hooks` in Codex and trust jouet's. Upgrading from a version that installed bare commands changes them, so they need trusting again. Cursor runs a project's hooks only in a trusted workspace, Gemini CLI runs hooks from settings files (yours too) only in a trusted folder, and Copilot reads a repository's hooks only once you trust the folder, and reads hooks only when it starts.

### When jouet is not on PATH

Each hook's command checks for jouet first and exits 0 silently when it is not on `PATH`, the way the Entire CLI's hooks do:

```sh
sh -c 'command -v jouet >/dev/null 2>&1 || exit 0; exec jouet hook claude pre-tool-use'
```

So a contributor without jouet, or an app started with a shorter `PATH` than your shell's, gets no hooks rather than an error on every tool call. setup says so when jouet is not on `PATH` where you run it. The command is `sh -c` because Codex runs hooks with your login shell, which may not be a POSIX one. On Windows, Claude Code runs hooks with Git Bash, which has `sh`; without Git Bash it falls back to PowerShell, where this command fails, as a bare `jouet` would when it is missing. Codex runs them with PowerShell, so its handlers also get a `commandWindows`: `if (Get-Command jouet -ErrorAction Ignore) { jouet hook codex pre-tool-use }`.

Cursor, Gemini CLI and Copilot read a hook's stdout as JSON even when it has nothing to say, so their guard prints `{}` instead: `sh -c 'command -v jouet >/dev/null 2>&1 || { echo {}; exit 0; }; exec jouet hook cursor stop'`. Copilot's handlers have a `bash` and a `powershell` command. Cursor and Gemini CLI have one command, which they run with PowerShell on Windows, so a `--global` install made on Windows gets the PowerShell form; a `--shared` one gets `sh -c`, for everyone else who clones the repository.

The hooks find jouet on `PATH` rather than at the path of the jouet that ran setup. That path changes on every upgrade with mise, Homebrew and other version managers, differs between machines that share a settings file, and means nothing to anyone else in a `--shared` install. A hook pointing at a path that is gone would skip just as silently, but on every machine after every upgrade.

### Worktrees

Claude Code reads `.claude/settings.local.json` from the main checkout's root, also in sessions that run in a linked worktree such as the desktop app's `.claude/worktrees/*`, so `--project` writes it there even when you run it from a worktree. The skill is read through from the main checkout only when the worktree has no `.claude/skills` directory of its own; a repository that tracks `.claude/skills` has one in every worktree. On Windows Claude Code reads the local file from each worktree instead, and a new worktree starts without it unless the repository's [`.worktreeinclude`](https://code.claude.com/docs/en/worktrees#copy-gitignored-files-into-worktrees) lists it.

If you work across many repositories or worktrees, install with `--global` instead, and choose which hooks run with the [settings](#settings) below.

### Cursor, Gemini CLI and Copilot

Each has documented command hooks for the four events jouet uses, and `jouet hook` reads their payloads into the shape the [rules](/rules) see: their shell tools become `Bash` (`Shell`, `run_shell_command`, `bash`), their file tools `Read`, `Grep`, `Edit` and `Write`, with Claude Code's field names, so a rule matches the same calls in every harness. What each cannot do:

| | Cursor | Gemini CLI | Copilot |
|---|---|---|---|
| Session start | `sessionStart` | `SessionStart` | `sessionStart` |
| Before a tool | `preToolUse`: `Shell`, `Read`, `Grep` | `BeforeTool`: `run_shell_command`, `read_file`, `grep_search`, `replace`, `write_file` | `preToolUse`: `bash`, `powershell`, `view`, `create`, `edit`, `grep`, `rg` |
| Rule actions | deny; warn after the call (`preToolUse` can only allow or deny, so the warning waits for `postToolUse`) | deny; warn after the call (`BeforeTool` can only allow or deny, so the warning waits for `AfterTool`) | deny; warn before the call, as `additionalContext` |
| After a tool | `postToolUse`, `postToolUseFailure` (`Shell`, `Read`, `Grep`); exit code from `tool_output` | `AfterTool`; exit code from the `Exit Code: N` line of the result, absent for 0 | `postToolUse`, `postToolUseFailure`; exit code from `<exited with exit code N>` in the result |
| After an edit | `afterFileEdit`, which formats with `[edit] fix_after` but cannot report it | `AfterTool`: `replace`, `write_file` | `postToolUse`: `create`, `edit` |
| Stop gate | `stop` blocks with a `followup_message`, Cursor's next user message: once (`loop_count`), and only for a completed turn | `AfterAgent` blocks (`decision: "block"`), once (`stop_hook_active`) | `agentStop` blocks (`decision: "block"`), once (`stop_hook_active`) |

- **Cursor** does not enforce a denial of a file read today (Cursor staff [confirmed](https://forum.cursor.com/t/154377) it is broken), so a rule on `Read` only warns there in effect. Its `Write` tool is not steered: it sends the whole file to hooks, which fails to start them for a large one. Cursor also runs the hooks in Claude Code's `settings.json` files: while jouet's Cursor hooks are installed, jouet's Claude Code hooks step aside in Cursor sessions (they tell by `CURSOR_VERSION` without `CLAUDECODE`), so each call gets one verdict. Cloud agents run only a repository's hooks, and not `sessionStart`.
- **Gemini CLI** gives the model a shell command's exit code only as text, so a command cancelled, killed by a signal or moved to the background is not recorded. Its matchers are unanchored regular expressions, so jouet's are anchored, and each handler is named (`jouet-pre-tool-use`, ...) so `hooksConfig.disabled` can turn one off.
- **Copilot**'s hooks reference does not list its tools' arguments; jouet reads `view`, `create` and `edit` by the names Copilot CLI sends today (`path`, `view_range`, `file_text`, `old_str`, `new_str`). A shell result without an exit code is not recorded. The cloud agent reads `.github/hooks/` from the default branch only, and its runner needs jouet on `PATH`. Copilot also reads a repository's `.claude/settings.json`; there jouet's Claude Code hooks have no way to tell Copilot from Claude Code, so a repository with both `--shared` installs runs each hook twice.
- **Warnings are never turned into denials.** A decision about a shell command, warnings included, goes to the [ledger](#the-ledger) when the call is checked, as in every harness, so `jouet stats` and winding see it whether or not the agent did. A warning held for after the call reaches the agent when the harness runs its post-tool-use hook for that call, which jouet installs for every steered tool, failures included; a call another hook denies never runs, and its warning stays in the ledger only.
- **Payloads** in `test/fixtures/hooks` follow each harness's documentation; jouet's tests replay them without the harness.

### opencode and Amp

`jouet setup` does not support these two, because neither has command hooks:

- **opencode** has only JavaScript and TypeScript plugins, and its V1 and V2 plugin APIs are incompatible.
- **Amp** has only TypeScript plugins, which replaced its `amp.permissions` rules with `delegate`, the one way it ran an outside program before a tool call.

jouet's commands work there all the same; only the hooks do not: no primer, rules or stop check. Add the snippet `jouet skill --format agents-md` prints to their `AGENTS.md`, which points the agent at `jouet agent-help`.

## The hooks

| Event | Claude Code | Codex | What jouet does |
|---|---|---|---|
| Session start | `SessionStart` | `SessionStart` | Adds a few lines of context: the command families by name, how to commit and edit with jouet, the rules that deny and what each suggests instead, in a fnox project how to use its secrets, and one line when this repository has rules that do not run until the user trusts them. |
| Before a shell command, a Read, a Grep or a file edit | `PreToolUse` (`Bash\|Read\|Grep\|Edit\|Write\|MultiEdit\|NotebookEdit`) | `PreToolUse` (`^Bash$`) | Runs the pre-tool-use rules: denies the command with one line of advice, or passes on a warning. |
| After a shell command or a Read | `PostToolUse` (`Bash\|Read`), `PostToolUseFailure` (`Bash`) | none | Runs the post-tool-use rules, which record test, lint and build runs, and remembers which lines of which files the session read. |
| After a file edit | `PostToolUse` (`Edit\|Write\|MultiEdit`) | none | With `[edit] fix_after`, formats the file with the project's hk fixers ([how](/commands/diag#formatting-after-edits)); otherwise nothing. |
| Stop | `Stop` | `Stop` | Blocks finishing once while a recorded check failed, when only partial test runs followed a change, or when files changed after the last hk check passed. With hk's stop hook merged in, its verdict too. |

Every hook reads the harness's JSON on stdin and **fails open**: input it cannot read, a broken config file, a bad `JOUET_DISPLAY`, a rule that throws or runs out of its budget, or a crash all exit 0 with no decision, so a problem in jouet never blocks the agent. Rule errors are reported on stderr. A pre-tool-use hook takes about 2 ms in a release build, starting the process included.

### Steering rules

The rules are TOML: built in, your organisation's in `/etc/jouet/rules`, which your settings cannot turn off, yours in `~/.config/jouet/rules`, the [rule packs](/rules#rule-packs) you pin by commit, and a repository's `.jouet/rules` once you trust it with `jouet rule trust`. Yours only warn unless they say `mode = "deny"`, and no rule of yours can replace or overrule a built-in one. Rules from outside run only once they pass their examples. [Rules](/rules) covers writing, testing and replacing them; `jouet rule list` shows which run. A rule only advises. jouet denies the command or warns about it and tells the agent what to do instead; it never rewrites a command. The agent sees the advice and the rule's name, for example:

```text
Stage explicit paths (`git add path/to/file ...`): `git add -A` and `git add .` sweep in caches, scratch files and other sessions' work. [jouet rule: git-add-all]
```

The built-in rules:

| Rule | Denies | Suggests |
|---|---|---|
| `sleep-poll` | `sleep N` together with `gh pr checks`, `gh run view` or a `statusCheckRollup` query | `jouet wait ci` |
| `ci-rerun` | `gh run rerun` (bare, `--failed` or `--job`), and `gh api` calls to a run's or job's `rerun` endpoints | [`jouet ci explain`](/commands/ci#jouet-ci-explain), whose `--rerun` reruns only a flaky or infrastructure failure |
| `review-threads` | `gh api graphql` (or `curl` to the GraphQL API) with a `reviewThreads` query | `jouet pr threads` |
| `python-edit` | a `python` heredoc or `-c` script that calls `.replace(` and writes a file | `jouet edit` |
| `git-add-all` | `git add -A`, `--all`, `.` or `:/` | staging explicit paths |
| `git-stash` | a bare `git stash`, `push` or `save` without a message, `pop`, `apply` or `drop` without a stash named, and `clear` | `git stash push -m <tag>` and `git stash apply <sha>`, or a WIP commit, because every worktree of a repository shares one stash |
| `no-verify` | `--no-verify` on any git command, and `git commit -n` | fixing what the hook reports |
| `force-push` | `git push --force`, `-f` or a `+refspec` without `--force-with-lease` | `--force-with-lease` |
| `reset-to-remote` | warns (does not deny) on `git checkout -B X START`, `git switch -C X START`, `git branch -f X START` and `git reset --hard origin/…` when the local branch has commits START lacks, such as fixes not yet pushed | `git merge START` on the branch, or [`jouet stack sync`](/commands/stack#syncing-a-stack) |
| `rebase-merged-base` | warns (does not deny) on `git rebase`, `gh stack rebase` and `gh stack sync` in a stacked branch whose prerequisite has merged: from gh-stack's metadata (the prerequisite's PR was submitted and its remote branch is gone), or with `rules.network`, from GitHub | [`jouet stack sync`](/commands/stack#syncing-a-stack), which merges the trunk in |
| `push-untested` | warns (does not deny) on `git push` and `gh pr create` when no full test run has passed at the current tree | running the full suite first, or saying why it is not needed |
| `print-secrets` | in a [fnox project](#fnox): bare `env`, `printenv`, `export -p` and `set`, `printenv NAME` and `echo $NAME` for a secret its `fnox.toml` defines; anywhere: `fnox get`, `fnox export` without `-o`, `fnox list -V` and `fnox env --json`, unless redirected, piped or captured | `fnox exec -- <command>` |
| `secret-files` | in a fnox project: reading `.env` or `.env.*` (not `.env.example`, `.sample`, `.template` or `.dist`), and the files fnox's `plain` providers read, with Read, `cat`, `grep`, `sed -n` and similar | `fnox exec -- <command>` |
| `scoped-lint` | warns (does not deny) on `cargo clippy` piped into `head` or `tail`, `eslint .`, and a bare `mise run lint`, unless it is the full lint: the work tree has nothing uncommitted (a check after a merge, pull or checkout), clippy denies warnings across the workspace or all targets as CI does, or the same call runs the test suite | `jouet diag check --changed` while iterating, and the full lint once at the end |
| `affected-tests` | warns (does not deny) on a whole-suite `pytest`, `jest`, `vitest run`, `npm test` or `mise run test` within 30 minutes of the last whole-suite run (unless that run's output was cut with `head`, `tail` or `grep`), in a repository where `jouet test --affected` can select | `jouet test --affected` while iterating, keeping the full run for the end |
| `piped-test-output` | warns (does not deny) on a test, build or e2e run (`cargo test`, `cargo build`, `go test`, `pytest`, `vitest`, `jest`, `bats`, `npm test`, `mise run test:e2e`, `make`, ...) whose output is piped into `head`, `tail`, `grep`, `rg` or `sed -n`, or written to a file that the same call reads back with one of them; not on lints (`scoped-lint`'s), `--help` or `--list`, or output left in a file | `jouet test --cmd -- CMD`, with the command filled in |
| `job-start` | a dev server or watcher (`npm run dev`, `vite`, `python -m http.server`, `uvicorn`, `rails s`, `cargo watch`, `docker compose up` and others), or a build or test suite (`cargo test`, `npm run build`, `pytest`, `make`, ...), started with `&` or `nohup ... &`, unless the same call `wait`s for it; warns on a server the harness runs in the background | `jouet job start ID -- CMD`, with the ID filled in, and for a server a ready check (`--http URL` the call goes on to poll, or `--port N` it is told or defaults to) |
| `job-ready` | `sleep` together with `curl`, `wget`, `http` or `nc` against `localhost`, `127.0.0.1`, `0.0.0.0` or `[::1]`, as in `sleep 5 && curl localhost:3000` or `until curl ...; do sleep 1; done` | `jouet job start ID --http URL -- CMD`, or for a server already running `jouet wait http URL` (`jouet wait port PORT` for one that does not speak HTTP) |
| `job-logs` | warns on `tail -f` of a `*.log` or `*.out` file, and on reading `nohup.out` | `jouet job logs ID -n 50` |
| `job-stop` | warns on `kill $(lsof ...)`, `lsof ... \| xargs kill`, `pkill`/`killall` of a dev server, `fuser -k` and `kill-port` | `jouet job stop ID` |
| `claimed-path` | warns (does not deny) on an edit (Edit, Write, MultiEdit, NotebookEdit), `git commit`, `jouet commit` or `jouet edit replace` that touches a path another live session [claims](/commands/claim), naming that session, its worktree and branch | leaving the path to that session, or coordinating first |
| `pitchfork-start` | starting by hand a daemon the project's pitchfork config declares: a command that is a daemon's `run` command (arguments added or not), or a server told to listen on a daemon's port | `pitchfork start ID`, with the daemon named |
| `pitchfork-stop` | `kill $(lsof ...)`, `lsof ... \| xargs kill`, `pkill`/`killall`, `fuser -k` or `kill-port` aimed at a declared daemon's port or command line | `pitchfork stop ID` or `pitchfork restart ID` |
| `pitchfork-logs` | warns on `pitchfork logs --follow` | `pitchfork logs ID -n 50` |
| `trust-rules` | `jouet rule trust` and `jouet rule pack add`, with or without `--yes`: a repository's rules put text in front of the agent, so the agent must not be the one to trust them | asking the user to run it in their own terminal |

The `pitchfork-start` and `pitchfork-stop` rules run only in a project with a pitchfork config (`pitchfork.toml`, `pitchfork.local.toml`, `.config/pitchfork.toml` or `.config/pitchfork.local.toml` in the call's directory or one above it), and read the daemons it declares from the files, without running pitchfork. They run before the `job-*` rules, so a declared daemon goes to pitchfork and anything else to `jouet job`; elsewhere the session-start primer leaves them out. The stop hook leaves pitchfork daemons alone: they are meant to keep running.

A rule that names a jouet command is only active when the installed jouet has that command, so the hooks never suggest something that is not there.

The rules read the command the way a shell does: quoted strings, here-document bodies and comments are data, so `git commit -m "never git add -A"` passes, while commands inside `$(...)` and `sh -c '...'` are checked. Wrappers such as `env`, `sudo`, `timeout`, `rtk` and `mise x ... --` are looked through.

### Finishing on a failing check

The stop hook keeps an agent from declaring success while a test, lint or build run it made failed, or after it only ran part of the test suite on changed code, in the spirit of "verification before completion". It is conservative by design:

- **What counts as a check.** Whatever the post-tool-use rules record. The built-in `checks` rule records `cargo test`, `build`, `check`, `clippy` and `fmt --check` (also through `mbx`), `mise run` tasks named `test*`, `lint*`, `check*`, `build*` or `ci`, `npm`/`pnpm`/`yarn`/`bun` test and scripts with those names, `make`/`just` targets, `go test`/`vet`/`build`, `pytest`, `python -m unittest`, `vitest`, `jest`, `bats`, `jouet test` (with `--cmd`, as the command it ran), `hk check` and a few common linters; add a [script rule](/rules#scripts) of your own with `on = "post-tool-use"` that calls `jouet.record` to count more.
- **Full or partial.** Each run is recorded with its scope: `full` when it covered the whole project, or `partial` with what it selected. For cargo, `-p` (unless the packages named are every workspace member), `--test`, `--lib`, `--bin`, `--exclude`, a test-name filter, `--exact` or `--skip` after `--`, and nextest's `-E` make a run partial; `--workspace`, a plain run in a single-package project, and a plain run at a virtual manifest without `default-members` are full. A plain `cargo test` at the root of a workspace that is also a package tests only that package, so it is partial. A task runner's task is full when it is run alone by a plain name (`mise run test`, `npm test`, `make check`), and partial with arguments or a sub-task (`mise run test:e2e edit`). `go test ./...`, and `pytest`, `vitest` or `jest` without paths or name filters, are full, as is `python -m unittest` with no tests named (or `discover` without a pattern or `-k`). `jouet test` is full, and `jouet test --affected` is partial, selecting `--affected` (see [`jouet test`](/commands/test)); `jouet test --cmd -- CMD` has the kind and scope of CMD, and is not recorded when the rule does not know CMD, as CMD run directly is not; a dry run is not recorded. Anything else, including a `cd` the rule cannot follow, is partial.
- **When a failure counts.** Only when the check is the command's last pipeline, so its exit status is the command's: `cd crate && cargo test` counts, `cargo test; echo done` does not. A jouet command that exits 2 (usage) or 4 (refused), such as `jouet diag check` where there is no hk config or `jouet test` with no runner it knows, checked nothing: it is not recorded, so it neither fails a check nor clears one.
- **What clears a failure.** A later full pass of the same kind, or a later pass of a partial run that selected the same thing (`cargo test -p a` failing, then `cargo test -p a` passing). A partial pass never clears a full failure: `cargo test -p a` passing after `cargo test --workspace` failed still blocks, and says only a full run clears it.
- **Partial runs after a change.** When the newest test run is a partial pass and the tree has changed since the last full test pass (or there has been none in the session), the hook blocks once and asks for the full suite, or for a reason it is not needed. Partial runs at the tree the full suite passed at are fine. Lint and build runs are not asked about.
- **The tree.** Each run records the tree it ran against: a hash of the content of every tracked file, staged or not, built from `git ls-files -s` and `git hash-object` (without `-w`) for files changed in the work tree, so nothing is written to the repository. Untracked and ignored files are left out, and a commit with the same content hashes the same, so committing what was tested keeps the tree. (`git stash create` gives a real tree id, but writes objects on every check.)
- **hk.** In a repository with an hk config, a passing hk check (`hk check`, `hk run check` or `jouet diag check`) covering the changed files is remembered with its tree; if files change after it, the hook asks for another check. A failing one is a lint failure like any other. See [one stop gate](/commands/diag#one-stop-gate) for running hk's own stop hook inside jouet's.
- **Jobs left running.** When the agent stops, the hook names the [`jouet job`](/commands/job)s that session started in this work tree that are still running, with the `jouet job stop` that would stop them. That is a note for the user (Claude Code's and Gemini CLI's `systemMessage`; Cursor and Copilot have no such note at a stop), never a reason to block; when a failed check blocks the stop, the note goes into the reason too. A job knows its session from `JOUET_SESSION_ID`, `CLAUDE_CODE_SESSION_ID` or `CODEX_THREAD_ID` when it was started.
- **Blocking once.** The hook blocks with a one-line reason, and if the agent stops again straight away (the harness reports `stop_hook_active`), its answer stands. A check that cannot pass never traps the agent.
- **Where runs are recorded.** Claude Code reports each command's exit code to the post-tool-use hooks, which keep each kind's newest run, newest full pass and uncleared failures in `$XDG_STATE_HOME/jouet/sessions/<session>.json`. Updates take a lock, so commands finishing at once cannot drop each other's runs, and files untouched for a week are removed. Cursor, Gemini CLI and Copilot report exit codes too, each in its own way ([above](#cursor-gemini-cli-and-copilot)), and their runs are kept the same way. Codex sends hooks no exit code, so its stop hook reads the commands in the last 8 MiB of the session transcript instead, and runs them through the same rules. That format is Codex's own: if it changes, the hook stops blocking rather than blocking wrongly. The transcript does not say which tree each command saw, so for Codex any partial test run after a full one asks for the full suite once.

### Before pushing and landing

The same records guard what leaves the machine:

- **`git push` and `gh pr create`.** The `push-untested` rule warns when no full test run has passed at the current tree: none is recorded, the last one failed, or the tree has changed since it passed. It stays quiet in a work tree where the hooks have never recorded a check, and on `git push --dry-run` and deletions.
- **`jouet pr land`** refuses (exit `4`) unless the ledger has a full test run that passed at the tree of the pull request's head. `--skip-verify REASON` lands it anyway, and the reason is kept in the ledger. See [`jouet pr land`](/commands/pr-land#tests-before-landing).

Known false positives and misses:

- A check you expected to fail, such as a test written to fail first or a failure that is already on the main branch, still blocks once. Say so in the reply and stop again.
- A full pass of a different selection does not clear a partial failure unless it is a full run: `cargo test -p a` failing, then `cargo test -p a -p b` passing, still blocks once.
- A check piped through `head`, `tail` or `grep` exits with that command's status, not the check's, so without `set -o pipefail` it is not recorded at all: it neither clears an earlier failure nor fails one. Run it through `jouet test` instead, or with `pipefail`.
- A command that changes tracked files and then runs the tests, such as a formatter in the same script, is recorded at the tree after it finished.
- Runs in the background, and checks run through a script the hook cannot see into, are not recorded.

### fnox

[fnox](https://fnox.jdx.dev) keeps a project's secrets encrypted or in a vault and hands them to programs as environment variables. In a project that uses it (a `fnox.toml`, `.fnox.toml` or `.config/fnox.toml` in the working directory or above), jouet helps keep the secrets out of the transcript, which leaves the machine with every turn, and out of commits:

- **Rules.** `print-secrets` and `secret-files` (in the table above) deny the usual ways an agent prints a secret, and suggest `fnox exec -- <command>`. The hook reads the secret names and the plain providers' files from the fnox config on every call, never a value. `env FOO=bar cmd`, `echo "$TOKEN" | gh auth login --with-token` and `fnox get NAME > file` stay allowed.
- **The primer.** The session-start context says that the project keeps its secrets in fnox. When the agent runs under `fnox proxy run`, it says so instead: its secret variables hold placeholders, which are not broken credentials. jouet detects the proxy by the `FNOX_PROXY_MCP=1` that `fnox proxy run` sets for its command.
- **Setup.** `jouet setup` recommends starting the agent with `fnox proxy run -- claude` (or `codex`). Under the proxy the agent's environment holds only placeholders, and fnox puts the real values into the HTTPS requests its `[[proxy.rules]]` allow, so a printed variable gives nothing away. See [fnox's credential proxy](https://fnox.jdx.dev/guide/proxy).
- **Commits.** [`jouet commit`](/commands/commit#secrets-in-fnox-projects) runs `fnox scan --values` over exactly what it is about to commit.

The rules are pattern matches, and they miss what they cannot see: a script that prints a variable, `python -c 'import os; print(os.environ)'`, or a value an agent copies out of a file under another name. The proxy is the stronger protection, because without it the values are in the agent's environment.

### The ledger

Every hook appends one compact JSON line to `$XDG_STATE_HOME/jouet/ledger/ledger.jsonl`. It holds the harness, the session id, a 16-character hash identifying the work tree (never its path), the time, the tool, the command's *shape*, the exit code and duration where the post-tool-use hooks report them, and each rule's decision (`deny`, `warn`, `record:<kind>`, or the stop check's `block`). A recorded check also carries its `scope` (`full` or `partial`, never what it selected) and the `tree` it ran against. Lines written before jouet kept these read as partial, with no tree. `jouet pr land --skip-verify` adds a `skip-verify` line with the reason given, credentials redacted. [Self-winding](/winding) measures each rule from it, and drafts new rules from the patterns in it.

```json
{"ts":1793380000,"harness":"claude","session":"s1","event":"pre-tool-use","tool":"Bash","shape":"git stash","decisions":[{"rule":"git-stash","action":"deny"}]}
{"ts":1793380090,"harness":"claude","session":"s1","event":"post-tool-use","tool":"Bash","shape":"cargo test -p <arg>","exit_code":101,"duration_ms":41200,"tree":"5d0c3f2a91b7e64c08f1a2b3c4d5e6f7","decisions":[{"rule":"checks","action":"record:test","scope":"partial"}]}
```

- **A shape never holds values.** It keeps each command's program, up to two of that program's known subcommands (`git stash push`, `gh pr checks`) and its flag names. Every other positional (paths, task and script names, `echo`'s words), flag values, message text, here-documents and environment assignments become placeholders, and anything token-like is redacted. `GH_TOKEN=... git -C /repo commit -m "fix it"` is recorded as `git -C <arg> commit -m <arg>`. A loop, conditional, `case` or subshell keeps its structure, with its list, patterns and names as placeholders: `for pr in 1 2; do gh pr checks $pr; done` is `for <arg> in <arg>…; do gh pr checks <arg>; done` (see [compound commands](/winding#compound-commands)).
- **Overrides.** When a command is denied or warned about and the session runs a command of the same shape within ten minutes, and it goes ahead this time, the ledger records an `override` of that rule. It is a heuristic for measuring rules later: same shape means the same program, subcommands and flags, whatever the values. Each denial or warning is counted once.
- **Rotation.** The file is rotated at 4 MiB, and rotated files are removed after 30 days.
- **Slow hooks.** A hook that runs over its time budget (`[hooks] budget_ms`, 500 ms by default for the tool hooks, ten times that for session start and stop) appends a `slow-hook` entry naming its event, how long it took and the budget, for [`jouet share`](/winding#what-sessions-show-about-jouet).
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
budget_ms = 500 # JOUET_HOOKS_BUDGET_MS: a tool hook slower than this is recorded as slow (session start and stop: 10×)

[rules]
disable = ["git-stash", "no-verify"] # JOUET_RULES_DISABLE=git-stash,no-verify; "all" for every rule but the enforced ones

[edit]
fix_after = false     # JOUET_EDIT_FIX_AFTER: format edited files with `hk run fix --safe`
fix_timeout_ms = 8000 # JOUET_EDIT_FIX_TIMEOUT_MS: then the fix is stopped
fix_steps = ["cargo-fmt"] # JOUET_EDIT_FIX_STEPS: only these hk steps (default: every fix step)

[stats]
summary = "quips" # JOUET_STATS_SUMMARY: quips, plain, or off for no line

[wind]
enabled = true # JOUET_WIND: self-winding, drafting rules from the ledger
```

An organisation's [enforced rules](/rules#enforced-rules) in `/etc/jouet/rules` (`JOUET_SYSTEM_DIR`) are not turned off by `disable`. Turning the hooks off still turns them off: they are policy over configuration, not a sandbox.

[Self-winding](/winding) is on after setup: the session-start primer shows at most two notes about rules jouet drafted from the ledger. `JOUET_WIND=0` turns it off.

## The primer, the skill and `agent-help`

The session-start primer is four lines, about 340 tokens: the command families by name, usage for `jouet test`, `jouet commit`, `jouet edit replace` and `jouet pr threads reply` with a note that these need no lookup, the rules that deny with the command each suggests, and the stop check. It leaves out what each command is for: that is one `jouet agent-help COMMAND` away, and every request in a session pays for the primer. Naming the deny rules up front spares the agent a denied call to learn them. It also says that Python scripts that rewrite files are denied, and that `jouet edit replace` takes several files and replacements in one call, because agents reached for a heredoc to batch replacements even with the `python-edit` rule named.

`jouet skill` prints the skill `jouet setup --skill` installs, about 25 lines of when and why that end in "run `jouet agent-help`". setup leaves it out by default because the primer covers it: with both, every request carried the command list twice, and Codex read the skill file before starting. `--format agents-md` prints the same text as a snippet for an `AGENTS.md`, for harnesses without skills or hooks.

`jouet agent-help` prints a guide for the whole tool in under 40 lines, one entry per command family with its subcommands by name, and `jouet agent-help COMMAND` one command's usage in about 1 kB: when to reach for it, the invocations agents use, each with what it does, the exit codes, and a last line saying what `--full` adds. `jouet agent-help COMMAND --full` prints the whole guide: that usage, then every subcommand with all its flags and details, and the output conventions. Agents used to read the whole guide, up to 7 kB, before a command's first use. With `--json`, `text` is the whole guide and `usage` the short one. Both are generated from the binary's own CLI definition, so they describe exactly the installed version. When [mbx](https://github.com/jdx/mr-boxington), rtk, beads (`bd`) or [fnox](https://fnox.jdx.dev) is on `PATH`, the guide says what each is for instead of jouet duplicating it.

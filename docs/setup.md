---
description: Install jouet's hooks into Claude Code, Codex, Cursor, Gemini CLI or Copilot, so agents are steered toward jouet's commands and cannot finish on a failing check.
---

# Agent setup and hooks

`jouet setup` connects jouet to a coding agent's harness by installing hooks that call back into `jouet hook`. When a session starts, the hooks add a few lines about jouet's commands. Before a shell command or file tool runs, they apply jouet's [rules](/rules), which deny a handful of costly hand-rolled patterns and name the jouet command or git habit to use instead. When the agent tries to finish, they block it once if a test, lint or build run failed, or if it changed files and no test run has passed since.

::: warning Early development
The hooks, their rules and the settings below are new and may change between releases.
:::

## Installing

Name the harness. Without a scope flag, setup installs for every project, under your home directory (`--global`):

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

Setup prints a line for each file it created, updated, removed, kept or left unchanged, then a `note:` for anything left for you to do:

```text
$ jouet setup codex
created /home/me/.codex/hooks.json
note: Codex skips new or changed hooks until they are trusted: run /hooks in Codex and trust jouet's.
```

Codex, Cursor, Gemini CLI and Copilot each have a step of their own before the hooks run. In Codex, run `/hooks` and trust jouet's; upgrading from a version that installed bare commands changes them, so they need trusting again. Cursor runs a project's hooks only in a trusted workspace. Gemini CLI runs hooks from settings files (yours too) only in a trusted folder. Copilot reads a repository's hooks only once you trust the folder, and reads hooks only when it starts, so restart Copilot CLI after setup.

The hooks find `jouet` on `PATH` and do nothing where it is missing; setup says so when it is not on `PATH` where you run it (see [when jouet is not on PATH](#when-jouet-is-not-on-path)). Setup does not install the [skill](#the-skill-and-agent-help) unless you pass `--skill`, or the hooks or the primer are turned off in the [settings](#settings): the primer already tells the agent what the skill would, in every session. In a [fnox](#fnox) project, setup also recommends starting the agent under fnox's credential proxy.

`--dry-run` prints each file's diff and writes nothing. With `--json`, the report is one document: `harness`, `scope` (`global` or `project`), `shared`, `dry_run` and `uninstall` when set, `files` (each with its `path`, an `action` of `create`, `update`, `remove`, `unchanged` or `keep`, and a `diff` when it changes), `notes`, and `benchmark_prompt` with [`--benchmark`](#compare-a-project).

### What setup changes

| Harness | Scope | Hooks | Skill |
|---|---|---|---|
| Claude Code | `--global` (default) | `~/.claude/settings.json` (`$CLAUDE_CONFIG_DIR` if set) | `~/.claude/skills/jouet/SKILL.md` (`$CLAUDE_CONFIG_DIR/skills` if set) |
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

- **`--project` is yours alone.** It writes at the root of the enclosing git repository (the current directory outside one), into Claude Code's `.claude/settings.local.json`, which Claude Code reads alongside `settings.json` but which is not meant to be committed. From a linked worktree it writes the main checkout's, except on Windows (see [worktrees](#worktrees)). Claude Code ignores that file in git once it has written to it itself; until then, and for the skill, setup says when git does not ignore them, so you can list them in `.gitignore` or `.git/info/exclude`.
- **`--shared` is for everyone who clones the repository.** It implies `--project` and writes the repository's tracked hooks file (the `--shared` rows above), for a team that has agreed to use jouet. Since the hooks do nothing where jouet is not installed, contributors without it are not broken, but they get no hooks either.
- **Only Claude Code has a project hooks file of your own.** Codex reads project hooks only from the repository's `.codex/hooks.json` and `.codex/config.toml`, Cursor from `.cursor/hooks.json`, Gemini CLI from `.gemini/settings.json` and Copilot from `.github/hooks/*.json`, so `--project` is refused for them (exit 2): use `--global`, or `--shared` to install there for everyone. `--global` together with `--project` or `--shared` is refused as well.
- **Merges, never clobbers.** jouet adds its own matcher group (in Cursor's and Copilot's files, its own handler) to each event and leaves every other hook, setting and key order as it was. A file that is not valid JSON, or whose `hooks` is not an object of arrays, is refused (exit 4) and left alone. A file that is a symlink, as dotfile managers make them, is written through to the file it names, keeping that file's permissions.
- **Re-running is a no-op.** jouet recognises its entries by their command, which runs `jouet hook ...`, and only rewrites them when they differ from what this version installs. Entries an older version installed, such as the bare `jouet hook claude stop`, are replaced in place, not duplicated.
- **hk's stop hook is merged in.** When the file already has hk's stop hook (`hk agent stop-hook`, as `hk agent hooks` installs it), setup folds it into jouet's stop handler, which runs it, so a stop gets one verdict. setup notes when another of the harness's hook files has hk's stop hook, which would still block on its own; see [one stop gate](/commands/diag#one-stop-gate).
- **Each handler has a 10-second timeout,** the bound on a hung hook, after which the harness carries on without it; jouet's hooks take milliseconds. A stop handler with hk's merged in gets hk's timeout instead, or 600 seconds when hk's had none.
- **`--uninstall` removes only jouet's entries**, then any group or event that removing them left empty, and puts a merged hk stop hook back as it was. At project scope it removes them from both `settings.local.json` and `settings.json`, whichever setup wrote. A settings file left empty stays, as `{}`; Copilot's `jouet.json`, which is jouet's alone, is removed.
- **The skill is jouet's only while it is unedited.** setup ends the `SKILL.md` it writes with a line holding a hash of the rest. It replaces or removes that file only while the hash still matches, and keeps any other `skills/jouet/SKILL.md`, including one you have edited. Running setup without `--skill` removes the skill an earlier version installed, while it is unedited.

## What the hooks do {#the-hooks}

| Event | Claude Code | Codex | What jouet does |
|---|---|---|---|
| Session start | `SessionStart` | `SessionStart` | Adds the [primer](#the-session-start-primer): the command families by name, usage for the common commands, the rules that deny and what each suggests instead, and notes such as the slow checks here, how to use a fnox project's secrets, or that this repository has rules and routines that do not run until the user trusts them. |
| Before a shell command, a Read, a Grep or a file edit | `PreToolUse` (`Bash\|Read\|Grep\|Edit\|Write\|MultiEdit\|NotebookEdit`) | `PreToolUse` (`^Bash$`) | Runs the pre-tool-use rules: denies the command with one line of advice, or passes on a warning. |
| After a shell command or a Read | `PostToolUse` (`Bash\|Read`), `PostToolUseFailure` (`Bash`) | none | Runs the post-tool-use rules, which record test, lint and build runs, keeps how long each check took for [`jouet cost`](/commands/cost), and remembers which lines of which files the session read. |
| After a file edit | `PostToolUse` (`Edit\|Write\|MultiEdit`) | none | With `[edit] fix_after`, formats the file with the project's hk fixers ([how](/commands/diag#formatting-after-edits)); otherwise nothing. |
| Stop | `Stop` | `Stop` | Blocks finishing once while a recorded test, lint or build run failed, when files changed after the last passing test run and no test run has passed since, or when files changed after the last hk check passed. With hk's stop hook merged in, its verdict too. |

Cursor, Gemini CLI and Copilot get the same hooks under their own event names; [Cursor, Gemini CLI and Copilot](#cursor-gemini-cli-and-copilot) lists them, and what each harness cannot do.

Every hook reads the harness's JSON on stdin and **fails open**: input it cannot read, a broken config file, a bad `JOUET_DISPLAY`, a rule that throws or runs out of its budget, or a crash all exit 0 with no decision, so a problem in jouet never blocks the agent. Rule errors are reported on stderr. A pre-tool-use hook takes about 2 ms in a release build, starting the process included.

### The session-start primer

The primer is four lines, about 470 tokens, and every request in a session pays for it. In the jouet repository, Claude Code's reads as follows; the last line is one of the notes described below:

> jouet is installed, a CLI toolkit for coding agents: claim, commit/secrets, diag, edit, handoff/pickup, pr, read/docs/deps/db, review-package, stack, search, test, wait/http, job, ci, rule, run, wind.
>
> Test with `jouet test --affected` (what your changes affect) or `jouet test --cmd -- CMD...` (any other test or build). Give a build or test that can take over 2 minutes a Bash `timeout` of 600000, so it ends in that call. Commit only the paths you name with `jouet commit <PATHS>... --message MSG`, in the same call as their test run (`jouet test ... && jouet commit ...`). Edit with your own edit tool or `jouet edit replace <PATHS>... --old TEXT --new TEXT`, which takes several files and repeated --old/--new pairs at once; never a Python script. Find code with `jouet search PATTERN [PATHS]` (the whole tree, matches grouped by the function they are in, within a budget) instead of `grep -rn`, and read one definition with `jouet read PATH --symbol NAME` instead of `sed -n` ranges. List a PR's open review threads with `jouet pr threads [PR]`; reply to and resolve one with `jouet pr threads reply <ID> [PR] --body TEXT --resolve`. These need no lookup; for other commands, `jouet agent-help COMMAND` gives their usage.
>
> jouet's hooks deny these and say what to do instead: sleep-poll (use `jouet wait ci`), ci-rerun (use `jouet ci explain`), review-threads (use `jouet pr threads`), python-edit (use `jouet edit`), git-add-all (use `git add path/to/file ...`), git-stash, no-verify, force-push (use `git push --force-with-lease`), print-secrets, secret-files (use `fnox exec -- <command>`), job-start, job-ready (use `jouet job start ID --http URL -- CMD`), trust.
>
> They also stop you finishing while a test, lint or build run failed, or when no test passed since your last change. For an unrelated failure, compare its command with `jouet test --baseline --cmd -- CMD`.
>
> jouet: slow checks here: `mise run test` ~3m44s, `mise run test:bats -- test/stats.bats` ~3m33s, `cargo build -q --release` ~2m30s, `bats test/hook.bats test/stack.bats` ~1m14s (`jouet cost` lists all). Finish the edits a slow run checks before starting it, so one run covers them.

The primer names the command families but leaves out what each is for, which is one `jouet agent-help COMMAND` away. Its other parts, and why they are there:

- **Common commands.** Usage for `jouet test`, `jouet commit`, `jouet edit replace`, `jouet search`, `jouet read --symbol`, and `jouet pr threads` with its `reply`, with a note that these need no lookup. `jouet test --affected` is named only where it can select tests.
- **Commit with the tests.** `jouet commit` goes in the same call as the test run, which saves agents a request of their own for it.
- **Search and read by symbol.** `jouet search` and `jouet read --symbol` are given with their usage, in place of `grep -rn` and `sed -n` ranges: agents never used them while the primer only named them, and spent most of an exploration-heavy task on greps.
- **No Python rewrites.** The primer says that Python scripts that rewrite files are denied, and that `jouet edit replace` takes several files and replacements in one call, because agents reached for a heredoc to batch replacements even with the `python-edit` rule named.
- **The rules that deny**, with the command each suggests, which spares the agent a denied call to learn them. Only rules that can fire here are named: the pitchfork rules only in a project with a pitchfork config.
- **The stop check**, while `[hooks] verify` is on, and how to compare an unrelated failure with the merge base.
- **Long runs in one call.** How the harness waits for a long build or test without a call per check. Claude Code: a Bash `timeout` of 600000, past which a call moves to the background. Codex: a large `yield_time_ms` or a loop on `write_stdin`, not a call per poll. Cursor, Gemini CLI and Copilot get no such line. In Claude Code's `bypassPermissions` mode the pre-tool-use hook also sets that timeout itself on a test, build or lint call that has none and is not run in the background, through `updatedInput`. That needs an `allow`, which in any other mode would skip the user's prompt, so it does this in no other mode. The command itself is never changed.

A note follows the primer only when it applies:

- where [`jouet cost`](/commands/cost) has timed checks of 20 seconds or more, the slowest, up to four, so the agent can finish the edits a slow run checks before starting it;
- at most two notes from [self-winding](/winding), such as a rule drafted from the ledger;
- when this repository has rules and routines that do not run until the user trusts them, how many, and that trusting them is the user's decision, in their own terminal; when a plugin is not fetched as pinned, that `jouet plugin sync` fetches it;
- in a [fnox](#fnox) project, how to use its secrets, or that the session runs under `fnox proxy run`;
- for each deprecated whole-file `.luau` rule, a line the agent can act on to convert it.

### Steering rules

The rules are TOML. They come from five places: built in; your organisation's in `/etc/jouet/rules`, which your settings cannot turn off; yours in `~/.config/jouet/rules`; the [plugins](/plugins) you pin by commit; and a repository's `.jouet/rules` once you trust it with `jouet trust`. Yours only warn unless they say `mode = "deny"`, and no rule of yours can replace or overrule a built-in one. Rules from outside run only once they pass their examples. [Rules](/rules) covers writing, testing and disabling them; `jouet rule list` shows which run.

A rule only advises. jouet denies the command or warns about it and tells the agent what to do instead, and never rewrites the command. The agent sees the advice and the rule's name:

```text
Run `git add path/to/file ...` instead. `git add -A` and `git add .` sweep in caches, scratch files and other sessions' work. [jouet rule: git-add-all]
```

The built-in rules:

| Rule | Denies | Suggests |
|---|---|---|
| `sleep-poll` | `sleep N` together with `gh pr checks`, `gh run view` or a `statusCheckRollup` query | `jouet wait ci` |
| `ci-rerun` | `gh run rerun` (bare, `--failed` or `--job`), and `gh api` calls to a run's or job's `rerun` endpoints | [`jouet ci explain`](/commands/ci#jouet-ci-explain), whose `--rerun` reruns only a flaky or infrastructure failure |
| `review-threads` | `gh api graphql` (or `curl` to the GraphQL API) with a `reviewThreads` query | `jouet pr threads` |
| `python-edit` | a `python` heredoc or `-c` script that reads a file and writes it back, by `.replace(`, slicing, a regex or anything else (a file written from another, a json, yaml or toml rewrite, or a script that stops on a miss with an `assert`, `raise` or `sys.exit` before it writes, is left alone). A script whose `.replace(` calls each take a plain literal is checked against the file instead: it runs when every old text is there, and is denied, naming the text, when one is not | `jouet edit` |
| `sed-line-edit` | warns (does not deny) on `sed -i` that deletes, changes or inserts by line number (`356,413d`, `12i\`): line numbers shift once one edit lands. Substitutions and pattern addresses are left alone | your edit tool, or `jouet edit replace` |
| `git-add-all` | `git add -A`, `--all`, `.` or `:/` | staging explicit paths |
| `git-stash` | a bare `git stash`, `push` or `save` without a message, `pop`, `apply` or `drop` without a stash named, and `clear` | `git stash push -m <tag>` and `git stash apply <sha>`, or a WIP commit, because every worktree of a repository shares one stash |
| `no-verify` | `--no-verify` on any git command, and `git commit -n` | fixing what the hook reports |
| `force-push` | `git push --force`, `-f` or a `+refspec` without `--force-with-lease` | `--force-with-lease` |
| `reset-to-remote` | warns (does not deny) on `git checkout -B X START`, `git switch -C X START`, `git branch -f X START` and `git reset --hard origin/…` when the local branch has commits START lacks, such as fixes not yet pushed | `git merge START` on the branch, or [`jouet stack sync`](/commands/stack#syncing-a-stack) |
| `rebase-merged-base` | warns (does not deny) on `git rebase`, `gh stack rebase` and `gh stack sync` in a stacked branch whose prerequisite has merged: from gh-stack's metadata (the prerequisite's PR was submitted and its remote branch is gone), or with `rules.network`, from GitHub | [`jouet stack sync`](/commands/stack#syncing-a-stack), which merges the trunk in |
| `print-secrets` | in a [fnox project](#fnox): bare `env`, `printenv`, `export -p` and `set`, `printenv NAME` and `echo $NAME` for a secret its `fnox.toml` defines; anywhere: `fnox get`, `fnox export` without `-o`, `fnox list -V` and `fnox env --json`, unless redirected, piped or captured | `fnox exec -- <command>` |
| `secret-files` | in a fnox project: reading `.env` or `.env.*` (not `.env.example`, `.sample`, `.template` or `.dist`), and the files fnox's `plain` providers read, with Read, `cat`, `grep`, `sed -n` and similar | `fnox exec -- <command>` |
| `committed-secrets` | `git commit` when the lines it would add (staged, every tracked change for `-a`, and the paths a `git add` in the same call names) hold a secret gitleaks' rules recognise, and `git push` when commits not on any remote add one; only warns when gitleaks' generic rule is all that matched; skipped for a diff over 1 MiB | taking the secret out; for a false positive, [`jouet commit --allow-secret RULE:PATH:LINE`](/commands/secrets#false-positives), `gitleaks:allow` or `.gitleaksignore` |
| `scoped-lint` | warns (does not deny) on `cargo clippy` piped into `head` or `tail`, `eslint .`, and a bare `mise run lint`, unless it is the full lint: the work tree has nothing uncommitted (a check after a merge, pull or checkout), clippy denies warnings across the workspace or all targets as CI does, or the same call runs the test suite | `jouet diag check --changed` while iterating, and the full lint once at the end |
| `affected-tests` | warns (does not deny) on a whole-suite `pytest`, `jest`, `vitest run`, `npm test` or `mise run test` within 30 minutes of the last whole-suite run (unless that run's output was cut with `head`, `tail` or `grep`), in a repository where `jouet test --affected` can select | `jouet test --affected` while iterating, keeping the full run for the end |
| `piped-test-output` | warns (does not deny) on a test, build or e2e run (`cargo test`, `cargo build`, `go test`, `pytest`, `vitest`, `jest`, `bats`, `npm test`, `mise run test:e2e`, `make`, ...) whose output is piped into `head`, `tail`, `grep`, `rg` or `sed -n` (not `jouet test`, which records its own result and prints little), or written to a file that the same call reads back with one of them; not on lints (`scoped-lint`'s), `--help` or `--list`, or output left in a file | `jouet test --cmd -- CMD`, with the command filled in |
| `unbudgeted-search` | warns (does not deny), in a repository of 200 tracked files or more, on `rg` or `grep -r` over the whole working directory with no `--max-count`, no `-l` or `-c`, and no pipe into `head`, `tail` or `wc`; and on Claude Code's Grep tool printing matching lines over the whole tree with no `head_limit` | [`jouet search PATTERN`](/commands/search#rules) |
| `reread` | warns (does not deny) on reading lines this session already read, from a file unchanged since: Claude Code's Read tool, or a shell read with `cat`, `head`, `tail`, `sed -n` or `nl` | using what it read earlier, or [`jouet search PATTERN FILE`](/commands/search#sessions), which shows only matches the session has not seen |
| `large-read` | warns (does not deny) on reading 400 lines or more of source code at once: Claude Code's Read with no `offset` or `limit`, or `cat` or `nl` | [`jouet read FILE --outline`](/commands/read#reading-a-file), then `--symbol NAME` |
| `unbounded-query` | warns (does not deny) on `sqlite3 DB "select …"`, `sqlite3 DB .dump` or `psql -c "select …"` printing to the transcript, with no `LIMIT` and not a single aggregate row | [`jouet db query`](/commands/db#a-query) |
| `curl-json` | warns (does not deny) on `curl` piped into `jq`, or into Python code that parses JSON (`python3 -c 'import json...'`, `python3 -m json.tool`, `python3 - <<EOF`); not on downloads to a file (`-o`, `-O`) or installers piped into `sh` | [`jouet http GET URL --select '$.path'`](/commands/http#steering) |
| `job-start` | a dev server or watcher (`npm run dev`, `vite`, `python -m http.server`, `uvicorn`, `rails s`, `cargo watch`, `docker compose up` and others), or a build or test suite (`cargo test`, `npm run build`, `pytest`, `make`, ...), started with `&` or `nohup ... &`, unless the same call `wait`s for it; warns on a server the harness runs in the background | `jouet job start ID -- CMD`, with the ID filled in, and for a server a ready check (`--http URL` the call goes on to poll, or `--port N` it is told or defaults to) |
| `job-ready` | `sleep` together with `curl`, `wget`, `http` or `nc` against `localhost`, `127.0.0.1`, `0.0.0.0` or `[::1]`, as in `sleep 5 && curl localhost:3000` or `until curl ...; do sleep 1; done` | `jouet job start ID --http URL -- CMD`, or for a server already running `jouet wait http URL` (`jouet wait port PORT` for one that does not speak HTTP) |
| `job-logs` | warns on `tail -f` of a `*.log` or `*.out` file, and on reading `nohup.out` | `jouet job logs ID -n 50` |
| `job-stop` | warns on `kill $(lsof ...)`, `lsof ... \| xargs kill`, `pkill`/`killall` of a dev server, `fuser -k` and `kill-port` | `jouet job stop ID` |
| `claimed-path` | warns (does not deny) on an edit (Edit, Write, MultiEdit, NotebookEdit), `git commit`, `jouet commit` or `jouet edit replace` that touches a path another live session [claims](/commands/claim), naming that session, its worktree and branch | leaving the path to that session, or coordinating first |
| `pitchfork-start` | starting by hand a daemon the project's pitchfork config declares: a command that is a daemon's `run` command (arguments added or not), or a server told to listen on a daemon's port | `pitchfork start ID`, with the daemon named |
| `pitchfork-stop` | `kill $(lsof ...)`, `lsof ... \| xargs kill`, `pkill`/`killall`, `fuser -k` or `kill-port` aimed at a declared daemon's port or command line | `pitchfork stop ID` or `pitchfork restart ID` |
| `pitchfork-logs` | warns on `pitchfork logs --follow` | `pitchfork logs ID -n 50` |
| `trust` | `jouet trust` and `jouet plugin add`, with or without `--yes`: a repository's rules put text in front of the agent and its routines run commands, so the agent must not be the one to trust them | asking the user to run it in their own terminal |

One more built-in rule, `checks`, neither denies nor warns: it records test, lint and build runs for the [stop check](#what-counts-as-a-check).

The `pitchfork-start` and `pitchfork-stop` rules run only in a project with a pitchfork config (`pitchfork.toml`, `pitchfork.local.toml`, `.config/pitchfork.toml` or `.config/pitchfork.local.toml` in the call's directory or one above it), and read the daemons it declares from the files, without running pitchfork. They run before the `job-*` rules, so a declared daemon goes to pitchfork and anything else to `jouet job`; elsewhere the session-start primer leaves them out. The stop hook leaves pitchfork daemons alone: they are meant to keep running.

A rule that names a jouet command is only active when the installed jouet has that command, so the hooks never suggest something that is not there.

The rules read the command the way a shell does: quoted strings, here-document bodies and comments are data, so `git commit -m "never git add -A"` passes, while commands inside `$(...)` and `sh -c '...'` are checked. Wrappers such as `env`, `sudo`, `timeout`, `rtk` and `mise x ... --` are looked through.

### Finishing on a failing check

The stop hook keeps an agent from declaring success while a test, lint or build run it made failed, or after it changed files with no test run passing since, in the spirit of "verification before completion". A passing partial run satisfies the hook, so the hook alone does not show that the full suite passed.

After a failed `cargo test`, the agent's next stop is blocked with this reason:

```text
The last test run failed (`cargo test` exited 101). Fix it and run it again before you finish, or say plainly why it cannot pass. If the failures are not from your change, `jouet test --baseline --cmd -- cargo test` runs it at the merge base too, and passes when each failure fails there as well. [jouet: verify]
```

It blocks once: if the agent stops again straight away, its answer stands. The rest of this section says how the hook decides.

#### What counts as a check

- **Recorded commands.** Whatever the post-tool-use rules record. The built-in `checks` rule records `cargo test`, `build`, `check`, `clippy` and `fmt --check` (also through `mbx`), `mise run` tasks named `test*`, `lint*`, `check*`, `build*` or `ci`, `npm`/`pnpm`/`yarn`/`bun` test and scripts with those names, `make`/`just` targets, `go test`/`vet`/`build`, `pytest`, `python -m unittest`, `vitest`, `jest`, `bats`, `jouet test` (with `--cmd`, as the command it ran), `hk check` and a few common linters; add a [script rule](/rules#scripts) of your own with `on = "post-tool-use"` that calls `jouet.record` to count more.
- **Full or partial.** Each run is recorded with its scope: `full` when it covered the whole project, or `partial` with what it selected. For cargo, `-p` (unless the packages named are every workspace member), `--test`, `--lib`, `--bin`, `--exclude`, a test-name filter, `--exact` or `--skip` after `--`, and nextest's `-E` make a run partial; `--workspace`, a plain run in a single-package project, and a plain run at a virtual manifest without `default-members` are full. A plain `cargo test` at the root of a workspace that is also a package tests only that package, so it is partial. A task runner's task is full when it is run alone by a plain name (`mise run test`, `npm test`, `make check`), and partial with arguments or a sub-task (`mise run test:e2e edit`). `go test ./...`, and `pytest`, `vitest` or `jest` without paths or name filters, are full, as is `python -m unittest` with no tests named (or `discover` without a pattern or `-k`). `jouet test` is full, and `jouet test --affected` is partial, selecting `--affected` (see [`jouet test`](/commands/test)). `jouet test --cmd -- CMD` has the kind and scope of CMD, read through wrappers such as `mise x --`, `env` and `timeout` as any command is, and is not recorded when the rule does not know CMD, as CMD run directly is not; a dry run is not recorded. Anything else, including a `cd` the rule cannot follow, is partial.
- **When a failure counts.** Only when the check is the command's last pipeline, so its exit status is the command's: `cd crate && cargo test` counts, `cargo test; echo done` does not. A jouet command that exits 2 (usage) or 4 (refused), such as `jouet diag check` where there is no hk config or `jouet test` with no runner it knows, checked nothing: it is not recorded, so it neither fails a check nor clears one.
- **`jouet test` keeps its own result.** Piped, as in `jouet test | tail`, a run exits with tail's status, so the hooks cannot tell how it went; Codex sends hooks no exit status at all. So `jouet test` records each run itself, in the session's state (found by `CLAUDE_CODE_SESSION_ID` or `JOUET_SESSION_ID`) and in the ledger. A dry run records nothing.

#### What clears a failure

- **A later pass.** A later full pass of the same kind, or a later pass of a partial run that selected the same thing (`cargo test -p a` failing, then `cargo test -p a` passing). A partial pass never clears a full failure: `cargo test -p a` passing after `cargo test --workspace` failed still blocks, and says only a full run clears it.
- **Failures that were there before.** `jouet test` compares a failing full suite with the merge base on its own, and passes when every failure fails there as well; what the merge base did is kept, so later runs compare without running it again (see [`--baseline`](/commands/test#failures-that-were-there-before-baseline)). When a full test run or a build fails, the hook's message suggests that run's own command with `--baseline`, through `--cmd` when it did not go through `jouet test`. When the merge base could not run the command either, the hook asks about its failures once, and not again until the command passes, whatever changes.
- **Failures the machine caused.** A failed run whose every failure blames the machine rather than the change does not hold up finishing: no D-Bus or keyring (`DBus error`, the Secret Service), a system library missing (a shared object that will not load, a pkg-config miss, `could not find system library`), no network (`could not resolve host`, `network is unreachable`), or an API's rate limit. The stop says each such failure once, in a note for you (`systemMessage`), with the line that blamed the machine. A missing command or file is not counted, since it is most often the change's own, such as a binary it did not build, and one failure that says nothing of the kind among them makes the whole run count. jouet reads a failed run's output from `jouet test`'s own log, or from Claude Code's `PostToolUseFailure` payload.
- **A run still going.** `jouet test` marks itself in the session's state while it runs. While its process is alive, test failures and partial runs do not hold up finishing: the run's result decides the next stop. A run killed before it records anything leaves a mark whose process is gone, which holds nothing up.
- **Back to files that passed.** A failure at other files than the ones on disk now does not count when a pass that would clear it ran at exactly these files. So checking that a test fails without a fix, then putting the fix back, does not leave finishing waiting on the failure that proved the test.

#### Changes after a pass

- **A change after the last pass.** When the newest test run passed and files have changed since, with no test run passing at the files now, the hook blocks once and asks for the tests the change affects (`jouet test --affected` where it can select, or the tests for the code changed), or for a reason none are needed. Any passing test run at the files now is enough, partial or full. Lint and build runs are not asked about, and nothing is asked in a session that has run no tests.
- **Formatting after a pass.** Files a formatter or a commit's hooks rewrote count as tested when the files before them were: the hooks keep the tree before and after a Bash call whose every command is a formatter (`cargo fmt`, `rustfmt`, `gofmt`, `go fmt`, `black`, `ruff format`, `prettier --write`, `hk fix`), a commit (`git commit`, `jouet commit`, whose hooks may fix what they check), `git add` or `git status`, with pipes into `tail` or `head`. So `cargo fmt` after the tests, or hk fixing a file as it commits, needs no second test run, and an hk check that passed before still counts. A call with anything else in it, or a `cd`, is not formatting. This applies to Claude Code, whose hooks run before and after each call.
- **The tree.** Each run records the tree it ran against: a hash of the content of every tracked file, staged or not, and of every untracked file that is not ignored, built from `git ls-files -s` and `git hash-object` (without `-w`) for files changed or added in the work tree, so nothing is written to the repository. A commit with the same content hashes the same, so committing what was tested, new files included, keeps the tree. Ignored files are left out, and so are untracked ones when there are more than 1,000 of them. A scratch file left in the repository and not ignored counts as a change. (`git stash create` gives a real tree id, but writes objects on every check.)
- **hk.** In a repository with an hk config, a passing hk check (`hk check`, `hk run check` or `jouet diag check`) covering the changed files is remembered with its tree; if files change after it, the hook asks for another check. A failing one is a lint failure like any other. See [one stop gate](/commands/diag#one-stop-gate) for running hk's own stop hook inside jouet's.

#### Work trees

- **Which work tree.** A run counts for the work tree it ran in, after any `cd` before it: `cd ~/src/app && cargo test --workspace` is a full run of `~/src/app`, whichever repository the session started in. Each work tree is judged by its own runs, and a block names each tree other than the session's own. After a `cd` jouet cannot follow (`cd "$DIR"`, `cd -`), the run is not recorded.
- **A work tree the session left.** A work tree other than the session's own is not asked about once the session has left it: nothing there is uncommitted (no staged or unstaged change, no untracked file that is not ignored), and the session has not run or started a check there for an hour. Its runs judged work that has since been committed and left, such as a branch that merged and moved on. Something uncommitted there (in a submodule too), or any check run there, brings it back. The session's own work tree is always judged. For Codex, whose runs are read from the transcript at each stop, a run's time is the one its transcript line records.
- **A work tree that is gone.** The checks of a work tree whose directory no longer exists, such as a removed worktree, are not judged.

#### How it blocks

- **Blocking once.** The hook blocks with a one-line reason, and if the agent stops again straight away (the harness reports `stop_hook_active`), its answer stands. A check that cannot pass never traps the agent.
- **Once per run.** A reason is raised once for the run it is about: a failure once for that failing run, and files changed after a pass once for that pass. Changing files afterwards, or other runs passing, does not raise it again; a newer run does: the same command failing at other files, or a newer pass followed by more changes. The same failure of the same files again is not new. One block raises every reason that applies in every work tree the session ran checks in, the session's own first, up to five a tree, and says how many wait for the next stop; a tree's reasons are never left for a later turn, which might not touch it. Commands in its message are cut to 80 characters, but each run is recorded whole, so a command it suggests, such as `--baseline`'s, runs. The session's state keeps what each tree was told. Outside a repository, where there is no tree to compare, and for Codex, whose runs are read again from the transcript at each stop, every turn's stop is judged afresh.
- **Jobs left running.** When the agent stops, the hook names the [`jouet job`](/commands/job)s that session started in this work tree that are still running, with the `jouet job stop` that would stop them. That is a note for the user (Claude Code's and Gemini CLI's `systemMessage`; Cursor and Copilot have no such note at a stop), never a reason to block; when a failed check blocks the stop, the note goes into the reason too. A job knows its session from `JOUET_SESSION_ID`, `CLAUDE_CODE_SESSION_ID` or `CODEX_THREAD_ID` when it was started.

#### Where runs are recorded

Claude Code reports each command's exit code to the post-tool-use hooks, which keep each kind's newest run, newest full pass and uncleared failures (the 16 newest of a kind) in `$XDG_STATE_HOME/jouet/sessions/<session>.json`. Updates take a lock, so commands finishing at once cannot drop each other's runs, and files untouched for a week are removed. Cursor, Gemini CLI and Copilot report exit codes too, each in its own way ([below](#cursor-gemini-cli-and-copilot)), and their runs are kept the same way.

Codex sends hooks no exit code, so its stop hook reads the commands in the last 8 MiB of the session transcript instead, and runs them through the same rules. That format is Codex's own: if it changes, the hook stops blocking rather than blocking wrongly. The transcript does not say which tree each command saw, so for Codex a change after the last passing test run is not asked about.

#### Known false positives and misses

- A check you expected to fail, such as a test written to fail first or a failure that is already on the main branch, still blocks once. Say so in the reply and stop again; later turns are not asked again until the tree changes or a check runs.
- A pass of a different partial selection clears a partial failure only when it is the same command over more names, with no options in either, or the failed selection without the plain names after it: `mise run test:e2e test_b test_a` passing clears `mise run test:e2e test_a` failing, and `cargo test -p a` passing clears `cargo test -p a name` failing, a name filter that narrowed it. For cargo and nextest, whose name filters match part of a name, a shorter filter covers a longer one: `cargo test affected::` clears `cargo test affected::cargo`, though not with `--exact`. `cargo test -p a -p b` passing does not clear `cargo test -p a` failing, which still blocks once.
- A check piped through `head`, `tail` or `grep` exits with that command's status, not the check's, so without `set -o pipefail` it is not recorded at all: it neither clears an earlier failure nor fails one. Run it through `jouet test` instead, or with `pipefail`.
- A command that changes tracked files and then runs the tests, such as a formatter in the same script, is recorded at the tree after it finished.
- Runs in the background, and checks run through a script the hook cannot see into, are not recorded.

### Before pushing and landing

jouet does not ask for a local full run before `git push`, `gh pr create` or [`jouet pr land`](/commands/pr-land#tests-before-landing), which merges only once the pull request's checks have passed. Check that the configured CI checks cover the tests the change needs.

### fnox

[fnox](https://fnox.jdx.dev) keeps a project's secrets encrypted or in a vault and hands them to programs as environment variables. In a project that uses it (a `fnox.toml`, `.fnox.toml` or `.config/fnox.toml` in the working directory or above), jouet helps keep the secrets out of the transcript, which leaves the machine with every turn, and out of commits:

- **Rules.** `print-secrets` and `secret-files` (in the [table above](#steering-rules)) deny the usual ways an agent prints a secret, and suggest `fnox exec -- <command>`. The hook reads the secret names and the plain providers' files from the fnox config on every call, never a value. `env FOO=bar cmd`, `echo "$TOKEN" | gh auth login --with-token` and `fnox get NAME > file` stay allowed.
- **The primer.** The session-start context says that the project keeps its secrets in fnox. When the agent runs under `fnox proxy run`, it says so instead: its secret variables hold placeholders, which are not broken credentials. jouet detects the proxy by the `FNOX_PROXY_MCP=1` that `fnox proxy run` sets for its command.
- **Setup.** `jouet setup` recommends starting the agent with `fnox proxy run -- claude` (or `codex`, `cursor-agent`, and so on). Under the proxy the agent's environment holds only placeholders, and fnox puts the real values into the HTTPS requests its `[[proxy.rules]]` allow, so a printed variable gives nothing away. See [fnox's credential proxy](https://fnox.jdx.dev/guide/proxy).
- **Commits.** [`jouet commit`](/commands/commit#secret-scanning) runs `fnox scan --values` over exactly what it is about to commit, beside gitleaks' rules, which it runs in every project.

The rules are pattern matches, and they miss what they cannot see: a script that prints a variable, `python -c 'import os; print(os.environ)'`, or a value an agent copies out of a file under another name. The proxy is the stronger protection, because without it the values are in the agent's environment.

### The ledger

Every hook appends one compact JSON line to `$XDG_STATE_HOME/jouet/ledger/ledger.jsonl`. A line holds the harness, the session id, a 16-character hash identifying the work tree (never its path), the time, the tool, the command's *shape*, the exit code and duration where the post-tool-use hooks report them, and each rule's decision (`deny`, `warn`, `record:<kind>`, or the stop check's `block`, under the rule name `verify`). A recorded check also carries its `scope` (`full` or `partial`, never what it selected) and the `tree` it ran against. Lines written before jouet kept these read as partial, with no tree. [Self-winding](/winding) measures each rule from the ledger, and drafts new rules from the patterns in it.

```json
{"ts":1793380000,"harness":"claude","session":"s1","event":"pre-tool-use","tool":"Bash","shape":"git stash","decisions":[{"rule":"git-stash","action":"deny"}]}
{"ts":1793380090,"harness":"claude","session":"s1","event":"post-tool-use","tool":"Bash","shape":"cargo test -p <arg>","exit_code":101,"duration_ms":41200,"tree":"5d0c3f2a91b7e64c08f1a2b3c4d5e6f7","decisions":[{"rule":"checks","action":"record:test","scope":"partial"}]}
```

A few commands add lines of their own: `jouet test` a `test-run` line for each run, hk's fixers after an edit a `fix` line with how many files they changed ([how](/commands/diag#formatting-after-edits)), and a command that uses something deprecated a `deprecated` line, so that a session is told about it once ([deprecations](/stability#deprecations)).

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
graph = true   # JOUET_HOOKS_GRAPH: re-index the files an agent edits in the code graph, when the repository has one
budget_ms = 500 # JOUET_HOOKS_BUDGET_MS: a tool hook slower than this is recorded as slow (session start and stop: 10×)

[rules]
disable = ["git-stash", "no-verify"] # JOUET_RULES_DISABLE=git-stash,no-verify; "all" for every rule but the enforced ones
network = false # JOUET_RULES_NETWORK: let rules in hooks use the GitHub probes (jouet.pr.*); off so a hook never waits on the network

[edit]
fix_after = false     # JOUET_EDIT_FIX_AFTER: format edited files with `hk run fix --safe`
fix_timeout_ms = 8000 # JOUET_EDIT_FIX_TIMEOUT_MS: then the fix is stopped
fix_steps = ["cargo-fmt"] # JOUET_EDIT_FIX_STEPS: only these hk steps (default: every fix step)

[stats]
summary = "quips" # JOUET_STATS_SUMMARY: quips, plain, or off for no line

[cost]
record = true # JOUET_COST_RECORD: keep how long each build, test and lint took as a git note on HEAD (see `jouet cost`)
sequences = false # JOUET_COST_SEQUENCES: record local session-linked jouet test timings (see `jouet cost --sequences`)

[test]
baseline = "auto" # JOUET_TEST_BASELINE: auto compares a failing run with the merge base on its own (see `jouet test`), off only with --baseline

[wind]
enabled = true # JOUET_WIND: self-winding, drafting rules from the ledger

[entire]
enabled = true         # JOUET_ENTIRE: read Entire where it is set up (pickup, sessions, --why, pr findings, Entire-hosted repositories)
timeout_ms = 5000      # JOUET_ENTIRE_TIMEOUT_MS: each `entire` call for status, sessions, checkpoints, trails, findings and discussions
why_timeout_ms = 15000 # JOUET_ENTIRE_WHY_TIMEOUT_MS: each `entire why` for --why
```

A broken file, or a value jouet cannot read, falls back to the default for what it broke, so a typo never stops a hook working. `[wind]` and `[share]` have more settings, for promoting rules, reviewing sessions and checking memories; they are listed under [self-winding's settings](/winding#settings).

An organisation's [enforced rules](/plugins#enforced-rules-and-routines) in `/etc/jouet` (`JOUET_SYSTEM_DIR`) are not turned off by `disable`. Turning the hooks off still turns them off: enforced rules take precedence over your configuration, but they are not a sandbox.

[Self-winding](/winding) is on after setup: the session-start primer shows at most two of its notes, such as a rule jouet drafted from the ledger. `JOUET_WIND=0` turns it off.

## The skill and `agent-help`

`jouet skill` prints the skill `jouet setup --skill` installs: about 30 lines saying when to reach for each command family, then how to work with jouet, starting with `jouet agent-help` for the installed version's commands. setup leaves the skill out by default because the primer covers it: with both, every request carried the command list twice, and Codex read the skill file before starting. `--format agents-md` prints the same text as a snippet for an `AGENTS.md`, for harnesses without skills or hooks.

`jouet agent-help` prints a guide for the whole tool in at most 40 lines of its own, one entry per command family with its subcommands by name. `jouet agent-help COMMAND` prints one command's usage in about 1 kB: when to reach for it, the invocations agents use, each with what it does, a `more:` line saying what `--full` adds, and the exit codes. `jouet agent-help COMMAND --full` prints the whole guide: that usage, then every subcommand with all its flags and details, and the output conventions. Agents used to read the whole guide, up to 7 kB, before a command's first use. With `--json`, `text` is the whole guide and `usage` the short one.

Both are generated from the binary's own CLI definition, so they describe exactly the installed version. When [mbx](https://github.com/jdx/mr-boxington), jactionlint, rtk, beads (`bd`), [fnox](https://fnox.jdx.dev) or pitchfork is on `PATH`, the guide adds a line saying what each is for, instead of jouet duplicating it.

## Comparing with and without jouet {#compare-a-project}

When you run setup yourself at a terminal and it changes a file, it asks `Would you like a prompt to compare this project with and without jouet? [y/N]`. Paste the prompt it prints into a new agent session in the project. The agent proposes a reproducible task, a starting commit and an independent check, and asks you to approve the task, agent and model, number of paired runs, and time and token budget before running anything. It runs each pair from the same starting point in separate checkouts and sessions, without copying your credentials or changing your global agent configuration, and compares correctness, tokens and wall time. If the results suggest jouet underperforms, it offers to draft an issue or discussion for your review, and posts nothing until you approve it.

`jouet setup HARNESS --benchmark` runs setup as usual and prints the prompt after its report (with `--json`, as `benchmark_prompt`); it cannot be combined with `--dry-run` or `--uninstall` (exit 2). Setup does not launch the benchmark or manage the agent's login. The question appears only when a person is at the terminal (stdin and stderr are terminals and no coding agent is detected), and never with `--json`, `--dry-run`, `--uninstall` or `--benchmark`, so unattended setup never prompts and its output stays the same.

## Harness details

The sections below are reference for how the hooks fit each harness.

### When jouet is not on PATH

Each hook's command checks for jouet first and exits 0 silently when it is not on `PATH`, the way the Entire CLI's hooks do:

```sh
sh -c 'command -v jouet >/dev/null 2>&1 || exit 0; exec jouet hook claude pre-tool-use'
```

So a contributor without jouet, or an app started with a shorter `PATH` than your shell's, gets no hooks rather than an error on every tool call. setup says so when jouet is not on `PATH` where you run it. A stop handler with hk's stop hook merged in runs hk's alone when jouet is missing.

The command is `sh -c` because Codex runs hooks with your login shell, which may not be a POSIX one. On Windows, Claude Code runs hooks with Git Bash, which has `sh`; without Git Bash it falls back to PowerShell, where this command fails, as a bare `jouet` would when it is missing. Codex runs them with PowerShell, so its handlers also get a `commandWindows`: `if (Get-Command jouet -ErrorAction Ignore) { jouet hook codex pre-tool-use }`.

Cursor, Gemini CLI and Copilot read a hook's stdout as JSON even when it has nothing to say, so their guard prints `{}` instead: `sh -c 'command -v jouet >/dev/null 2>&1 || { echo {}; exit 0; }; exec jouet hook cursor stop'`. Copilot's handlers have a `bash` and a `powershell` command. Cursor and Gemini CLI have one command, which they run with PowerShell on Windows, so a `--global` install made on Windows gets the PowerShell form; a `--shared` one gets `sh -c`, for everyone else who clones the repository.

The hooks find jouet on `PATH` rather than at the path of the jouet that ran setup. That path changes on every upgrade with mise, Homebrew and other version managers, differs between machines that share a settings file, and means nothing to anyone else in a `--shared` install. A hook pointing at a path that is gone would skip as silently, but on every machine after every upgrade.

### Worktrees

Claude Code reads `.claude/settings.local.json` from the main checkout's root, also in sessions that run in a linked worktree such as the desktop app's `.claude/worktrees/*`, so `--project` writes it there even when you run it from a worktree. The skill is read through from the main checkout only when the worktree has no `.claude/skills` directory of its own; a repository that tracks `.claude/skills` has one in every worktree. On Windows Claude Code reads the local file from each worktree instead, and a new worktree starts without it unless the repository's [`.worktreeinclude`](https://code.claude.com/docs/en/worktrees#copy-gitignored-files-into-worktrees) lists it.

If you work across many repositories or worktrees, install with `--global` instead, and choose which hooks run with the [settings](#settings).

### Cursor, Gemini CLI and Copilot

Each has documented command hooks for the four events jouet uses, and `jouet hook` reads their payloads into the shape the [rules](/rules) see: their shell tools become `Bash` (`Shell`, `run_shell_command`, `bash`, `powershell`), their file tools `Read`, `Grep`, `Edit` and `Write`, with Claude Code's field names, so a rule matches the same calls in every harness. What each cannot do:

| | Cursor | Gemini CLI | Copilot |
|---|---|---|---|
| Session start | `sessionStart` | `SessionStart` | `sessionStart` |
| Before a tool | `preToolUse`: `Shell`, `Read`, `Grep` | `BeforeTool`: `run_shell_command`, `read_file`, `grep_search`, `search_file_content`, `replace`, `write_file` | `preToolUse`: `bash`, `powershell`, `view`, `create`, `edit`, `str_replace_editor`, `grep`, `rg` |
| Rule actions | deny; warn after the call (`preToolUse` can only allow or deny, so the warning waits for `postToolUse`) | deny; warn after the call (`BeforeTool` can only allow or deny, so the warning waits for `AfterTool`) | deny; warn before the call, as `additionalContext` |
| After a tool | `postToolUse`, `postToolUseFailure` (`Shell`, `Read`, `Grep`); exit code from `tool_output` | `AfterTool`; exit code from the `Exit Code: N` line of the result, absent for 0 | `postToolUse` (`bash`, `powershell`, `view`), `postToolUseFailure` (`bash`, `powershell`); exit code from `<exited with exit code N>` in the result |
| After an edit | `afterFileEdit`, which formats with `[edit] fix_after` but cannot report it | `AfterTool`: `replace`, `write_file` | `postToolUse`: `create`, `edit`, `str_replace_editor` |
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

jouet's commands work there, but without hooks there is no primer, no steering and no stop check. Add the snippet `jouet skill --format agents-md` prints to their `AGENTS.md`; it points the agent at `jouet agent-help`.

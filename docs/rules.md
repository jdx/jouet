---
description: Luau rules that read the command an agent is about to run, or has just run, and deny it, warn about it, or record it as a check.
---

# Rules

A jouet rule is a small [Luau](https://luau.org) script. It reads a tool call, such as the shell command an agent is about to run, and decides what to do: deny it with one line of advice, let it run with a warning, or say nothing. A rule that runs after a command can instead record it as a test, lint or build run.

jouet ships its own rules, for instance one that denies `git add -A` and one that denies a bare `git stash`. They are written against the same API as yours. You, or your agent, can write new ones, test them, and let them warn before they ever block anything.

::: warning Early development
The rule API is new and may change between releases. `jouet rule types` always describes the installed version.
:::

```console
$ jouet rule list
sleep-poll      enabled  deny    builtin  builtin  2026-10-30  A sleep loop around a CI query spends a model turn on every check; `jouet wait ci` blocks until the checks settle.
review-threads  enabled  deny    builtin  builtin  2026-10-30  Raw `reviewThreads` GraphQL is long and easy to get wrong; `jouet pr threads` lists, replies to and resolves threads.
python-edit     enabled  deny    builtin  builtin  2026-10-30  A Python heredoc that calls `.replace(` and writes the file back never checks that the text matched; `jouet edit` does.
git-add-all     enabled  deny    builtin  builtin  2026-10-30  `git add -A` and `git add .` stage everything, including caches, scratch files and other sessions' work.
git-stash       enabled  deny    builtin  builtin  2026-10-30  Every worktree of a repository shares one stash, so a bare stash, pop or drop can take another session's work.
no-verify       enabled  deny    builtin  builtin  2026-10-30  Skipping git hooks with `--no-verify` hides what they would have caught.
force-push      enabled  deny    builtin  builtin  2026-10-30  A plain force push overwrites commits pushed since you last fetched; `--force-with-lease` refuses instead.
checks          enabled  record  builtin  builtin  2026-10-30  Records test, lint and build runs, so jouet can stop an agent finishing while the last one failed.
```

The columns are: name, state, mode, scope, author, created, and why.

## Where rules come from

| Source | Directory | Runs | Mode unless the header says |
|---|---|---|---|
| Built in | embedded in the binary | always | deny |
| Yours | `~/.config/jouet/rules/*.luau` (`$XDG_CONFIG_HOME/jouet/rules`) | always | warn |
| A repository's | `.jouet/rules/*.luau` at the repository root | once trusted with `jouet rule trust` | deny |

A rule's name is its file name without `.luau`.

Rules of yours and a repository's **only add decisions**. jouet enforces this, so a rule written in a hurry, by you or an agent, cannot weaken the safety it already has:

- **No replacing.** A rule with the name of a built-in rule is refused (`jouet rule list` shows it as broken), and the built-in rule keeps running.
- **No overruling.** Rules run built-in rules first, and the first denial wins. `jouet.allow()` decides nothing for any other rule.
- **No locking out.** A denial of a command that only runs `jouet` becomes a warning, so a bad rule never stops you running `jouet rule list` or `jouet setup --uninstall` to fix it.
- **The sandbox.** It holds for every rule (see below).

A repository's rules can deny commands and put text in front of the agent, so they only run once you trust them. `jouet rule trust` records a hash of every rule file in `.jouet/rules`. Adding or changing a rule makes the directory untrusted again, and `jouet rule list` says so. `jouet rule untrust` withdraws trust.

To turn rules off without deleting them, list them in `~/.config/jouet/config.toml`, or in `JOUET_RULES_DISABLE` (comma-separated):

```toml
[rules]
disable = ["git-stash", "no-verify"] # "all" turns every rule off
```

## Writing a rule

A rule file starts with a header and returns a table with a `check` function. `check` gets the tool call and returns `jouet.deny(message)`, `jouet.warn(message)`, `jouet.allow()`, or `nil` to say nothing:

```lua
--!strict
--- why: rm -r deletes for good, and agents are quick to reach for it
--- mode: warn
--- author: jdx
--- created: 2026-10-30
local rule: JouetRule = {
	check = function(call: JouetCall): JouetDecision?
		for _, command in call.commands do
			if command.program == "rm" and jouet.short_flag(command.argv[2] or "", "r") then
				return jouet.deny("rm -r deletes for good; is this path right?")
			end
		end
		return nil
	end,
	examples = {
		warn = { "rm -rf target", "sudo rm -r build" },
		allow = { "rm file.txt", "echo 'rm -rf /'", "git commit -m 'rm -rf old code'" },
	},
}

return rule
```

The header is a few `--- key: value` lines at the top of the file, which `jouet rule list` shows without running the rule:

| Key | Meaning |
|---|---|
| `why` | Required. What the rule is for, in one line. |
| `mode` | `warn` or `deny`. A `warn` rule's denials are shown to the agent as warnings and the command runs. A rule of yours defaults to `warn`, so a new rule can prove itself before it blocks anything. |
| `name` | Optional; must match the file name. |
| `scope` | Optional `builtin`, `user` or `repo`; must match where the file lives. |
| `author` | Who wrote it: a person, or the agent session that did. |
| `created` | When. |

`call.commands` is the command parsed the way a shell parses it, so a rule never has to pick apart quoting itself:

- **Quoted text, here-document bodies and comments are data.** In `git commit -m "never git add -A"`, the only command is `git commit`.
- **Commands are split at `;`, `&&`, `||`, `|` and newlines.** Leading reserved words such as `do` and `then` are dropped, along with variable assignments and redirections.
- **Commands inside `$(...)` and in scripts passed to `sh -c` or `bash -lc` are included**, marked `nested`.
- **Wrappers are looked through:** `argv` skips `env FOO=1`, `sudo`, `timeout 60`, `nohup` and `mise x tool --`, and `program` is the basename of what runs.
- **rtk is looked through too.** [rtk](https://github.com/rtk-ai/rtk) compresses command output, and its hook rewrites `git status` to `rtk git status`. jouet's rules see the wrapped command, so `rtk git add -A` is denied like `git add -A`. A command it wrapped has `via = "rtk"`, and so does the call. rtk's own commands, such as `rtk gain`, are left alone.

The `jouet` module has helpers for the common cases:
- `jouet.git(command)` returns git's subcommand and arguments, past global options such as `-C dir`.
- `jouet.gh(command)` returns gh's arguments.
- `jouet.options(args)` returns the arguments before `--`.
- `jouet.short_flag(arg, "f", "o")` reads short-option clusters such as `-fu`.
- `jouet.has_command("wait", "ci")` asks whether the installed jouet has a command.

`jouet rule types` prints the whole API as Luau type definitions.

A rule that names a jouet command in its advice declares it with `needs = { "wait", "ci" }`, and stays off until the installed jouet has that command.

A rule with `on = "post-tool-use"` runs after a command finished. Each top-level command then has a `status`: the exit status it is known to have had. A failure belongs only to the last pipeline, so `cargo test; echo done` failing gives `cargo test` no status. Success belongs to every command in the final `&&` chain. `jouet.record("test", command)` records a run with that status, and the built-in `checks` rule uses it so jouet can refuse to let an agent finish while its last test, lint or build run failed.

### Testing a rule

`jouet rule test` runs a rule's `examples` and fails if any does not do what it expects. Examples check what `check` decides, before the mode applies, so a warn-mode rule's `deny` examples pass when it denies; the result for one `--command` is what jouet would enforce:

```console
$ jouet rule test ./no-rm-rf.luau
ok    warn          rm -rf target
ok    warn          sudo rm -r build
ok    allow         rm file.txt
ok    allow         echo 'rm -rf /'
ok    allow         git commit -m 'rm -rf old code'
no-rm-rf: 5 of 5 examples passed
```

It also runs a rule against one command or a recorded hook payload:

```sh
jouet rule test git-stash --command 'git stash pop'
jouet rule test checks --command 'cargo test' --exit-code 101
jouet rule test no-rm-rf --payload hook-input.json
```

### Type checking

Rules can be type-checked with [luau-lsp](https://github.com/JohnnyMorganz/luau-lsp) against the definitions jouet prints:

```sh
jouet rule types > jouet.d.luau
luau-lsp analyze --definitions=jouet.d.luau ~/.config/jouet/rules/*.luau
```

## The sandbox and its limits

Each rule runs in a sandboxed Luau VM:

- **What a rule can reach.** The standard library, without `require`, plus the `jouet` module. Luau has no `io`, and its `os` is limited to the clock. The `jouet` module and the libraries are read-only, and every rule has globals of its own.
- **Budgets.** Every call to a rule's `check`, and every loading of its file, has a 50 ms time budget and a budget of a million interrupts. Luau raises an interrupt at every function call and loop iteration. The whole VM may use 32 MiB.
- **Failing open.** A rule that does not compile, throws, runs out of budget or returns something unexpected says nothing. jouet reports it and carries on with the other rules.
- **Compiled-rule cache.** Compiled rules are cached in `$XDG_CACHE_HOME/jouet/luau`, keyed by a hash of their source. Luau does not verify bytecode, so that directory must stay writable only by you.

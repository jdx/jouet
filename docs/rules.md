---
description: Luau rules that read the command an agent is about to run, or has just run, and deny it, warn about it, or record it as a check.
---

# Rules

A jouet rule is a small [Luau](https://luau.org) script. It reads a tool call, such as the shell command an agent is about to run, and decides what to do: deny it with one line of advice, let it run with a warning, or say nothing. A rule that runs after a command can instead record it as a test, lint or build run.

jouet ships its own rules, for instance one that denies `git add -A` and one that denies a bare `git stash`. They are written against the same API as yours, and you can replace any of them. You, or your agent, can write new ones and test them before turning them on.

::: warning Early development
The rule API is new and may change between releases. `jouet rule types` always describes the installed version.
:::

```console
$ jouet rule list
sleep-poll      needs jouet wait ci     builtin  Deny sleep loops around `gh pr checks` and `gh run view`; use `jouet wait ci`
review-threads  needs jouet pr threads  builtin  Deny raw `reviewThreads` GraphQL; use `jouet pr threads`
python-edit     enabled                 builtin  Deny Python heredoc rewrites with `.replace(`; use `jouet edit`
git-add-all     enabled                 builtin  Deny `git add -A`, `--all` and `.`; stage explicit paths
git-stash       enabled                 builtin  Deny a bare `git stash`, `pop`, `drop` or `clear`; the stash is shared across worktrees
no-verify       enabled                 builtin  Deny `--no-verify` and `git commit -n`
force-push      enabled                 builtin  Deny force pushes without `--force-with-lease`
checks          enabled                 builtin  Record test, lint and build runs for the check before stopping
```

## Where rules come from

| Source | Directory | Runs |
|---|---|---|
| Built in | embedded in the binary | always |
| Yours | `~/.config/jouet/rules/*.luau` (`$XDG_CONFIG_HOME/jouet/rules`) | always |
| A repository's | `.jouet/rules/*.luau` at the repository root | once trusted with `jouet rule trust` |

A rule's name is its file name without `.luau`. A rule of yours, or a trusted repository's, with the name of a built-in rule replaces it. That is how you change a built-in rule: copy it from the [source](https://github.com/jdx/jouet/tree/main/crates/jouet-rules/rules) and edit it.

A repository's rules can deny commands and put text in front of the agent, so they only run once you trust them. `jouet rule trust` records a hash of every rule file in `.jouet/rules`. Adding or changing a rule makes the directory untrusted again, and `jouet rule list` says so. `jouet rule untrust` withdraws trust.

To turn rules off without deleting them, list them in `~/.config/jouet/config.toml`, or in `JOUET_RULES_DISABLE` (comma-separated):

```toml
[rules]
disable = ["git-stash", "no-verify"] # "all" turns every rule off
```

## Writing a rule

A rule file returns a table with a `description` and a `check` function. `check` gets the tool call and returns `jouet.deny(message)`, `jouet.warn(message)`, `jouet.allow()`, or `nil` to say nothing:

```lua
--!strict
-- ~/.config/jouet/rules/no-rm-rf.luau
local rule: JouetRule = {
	description = "Warn before rm -rf",
	check = function(call: JouetCall): JouetDecision?
		for _, command in call.commands do
			if command.program == "rm" and jouet.short_flag(command.argv[2] or "", "r") then
				return jouet.warn("rm -r deletes for good; is this path right?")
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

`call.commands` is the command parsed the way a shell parses it, so a rule never has to pick apart quoting itself:

- **Quoted text, here-document bodies and comments are data.** In `git commit -m "never git add -A"`, the only command is `git commit`.
- **Commands are split at `;`, `&&`, `||`, `|` and newlines.** Leading reserved words such as `do` and `then` are dropped, along with variable assignments and redirections.
- **Commands inside `$(...)` and in scripts passed to `sh -c` or `bash -lc` are included**, marked `nested`.
- **Wrappers are looked through:** `argv` skips `env FOO=1`, `sudo`, `timeout 60`, `nohup`, `rtk` and `mise x tool --`, and `program` is the basename of what runs.

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

`jouet rule test` runs a rule's `examples` and fails if any does not do what it expects:

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

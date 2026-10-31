---
description: Rules that read the command an agent is about to run, or has just run, and deny it, warn about it, or record it as a check.
---

# Rules

A jouet rule reads a tool call, such as the shell command an agent is about to run, and decides what to do: deny it with one line of advice, let it run with a warning, or say nothing. A rule is configuration, a `[[rule]]` table in a TOML file: which commands it matches, what it tells the agent, and examples that prove it. A rule with logic a matcher cannot express hands the decision to a small [Luau script](#scripts).

jouet ships its own rules, for instance one that denies `git add -A` and one that denies a bare `git stash`. They are written in the same format as yours. You, or your agent, can write new ones, test them, and let them warn before they ever block anything.

::: warning Early development
The rule format is new and may change between releases. `jouet rule schema` always describes the installed version.
:::

```output
$ jouet rule list
sleep-poll         enabled  deny    builtin  builtin  2026-10-30  A sleep loop around a CI query spends a model turn on every check; `jouet wait ci` blocks until the checks settle.
review-threads     enabled  deny    builtin  builtin  2026-10-30  Raw `reviewThreads` GraphQL is long and easy to get wrong; `jouet pr threads` lists, replies to and resolves threads.
python-edit        enabled  deny    builtin  builtin  2026-10-30  A Python heredoc that calls `.replace(` and writes the file back never checks that the text matched; `jouet edit` does.
git-add-all        enabled  deny    builtin  builtin  2026-10-30  `git add -A` and `git add .` stage everything, including caches, scratch files and other sessions' work.
git-stash          enabled  deny    builtin  builtin  2026-10-30  Every worktree of a repository shares one stash, so a bare stash, pop or drop can take another session's work.
no-verify          enabled  deny    builtin  builtin  2026-10-30  Skipping git hooks with `--no-verify` hides what they would have caught.
force-push         enabled  deny    builtin  builtin  2026-10-30  A plain force push overwrites commits pushed since you last fetched; `--force-with-lease` refuses instead.
print-secrets      enabled  deny    builtin  builtin  2026-10-31  Printing secret values puts them in the transcript, which leaves the machine with every turn and outlives the session.
secret-files       enabled  deny    builtin  builtin  2026-10-31  Reading a plaintext secrets file puts every value in it in the transcript, which leaves the machine with every turn and outlives the session.
push-untested      enabled  warn    builtin  builtin  2026-10-30  Pushing a tree that no full test run has passed at hands CI and reviewers untested code; a partial run does not count.
scoped-lint        enabled  warn    builtin  builtin  2026-10-30  While iterating, linting the whole project re-reports every file and costs minutes and pages of output; checking only the changed files through hk gives their findings as file:line:rule.
affected-tests     enabled  warn    builtin  builtin  2026-10-30  Running the whole test suite again after each small change spends minutes per run; while iterating, `jouet test --affected` runs only the tests the change affects.
pitchfork-start    enabled  deny    builtin  builtin  2026-10-31  Starting by hand a server the project's pitchfork config declares runs a second copy beside pitchfork's, without its ready check, log or restarts; `pitchfork start` runs the declared one.
pitchfork-stop     enabled  deny    builtin  builtin  2026-10-31  Killing a daemon pitchfork runs, by its port or its command line, looks to pitchfork like a crash and skips its graceful stop; `pitchfork stop` or `restart` does it by name.
pitchfork-logs     enabled  warn    builtin  builtin  2026-10-31  Following a daemon's log with `pitchfork logs --follow` never returns to the agent; `-n` prints the end of it and returns.
job-start          enabled  deny    builtin  builtin  2026-10-31  A server or a build started with `&` or nohup has no ready check, no log to ask for and no name to stop it by, so the agent polls it; `jouet job start` gives it all three.
job-ready          enabled  deny    builtin  builtin  2026-10-31  Sleeping and then fetching a local URL guesses how long a server takes to start; a ready check waits exactly that long, and stops early when the server dies.
job-logs           enabled  warn    builtin  builtin  2026-10-31  Following a log never returns to the agent, and reading a nohup log depends on where it was redirected; a job's log is one command away by its name.
job-stop           enabled  warn    builtin  builtin  2026-10-31  Killing a server by its port or its command line can hit the wrong process, or leave its children running; `jouet job stop` stops a job's whole process group by name.
claimed-path       enabled  warn    builtin  builtin  2026-10-31  Two sessions changing the same files on one machine overwrite or commit each other's work; `jouet claim` says who is working where.
checks             enabled  record  builtin  builtin  2026-10-30  Records test, lint and build runs and how much each covered, so jouet can stop an agent finishing while one failed or only part of the suite ran.
reread             enabled  warn    builtin  builtin  2026-10-30  Reading lines this session has already read, from a file unchanged since, puts the same text in the context twice.
large-read         enabled  warn    builtin  builtin  2026-10-30  Reading a long file whole to find one thing puts hundreds of lines in the context; its outline, then one definition, costs a fraction of that.
unbudgeted-search  enabled  warn    builtin  builtin  2026-10-30  A search of the whole tree with no limit can print thousands of lines into the context; `jouet search` stops at a budget and says what it left out.
```

The columns are: name, state, mode, scope, author, created, and why.

## Where rules come from

| Source | Directory | Runs | Mode unless the rule says |
|---|---|---|---|
| Built in | embedded in the binary | always | deny |
| Yours | `~/.config/jouet/rules/*.toml` (`$XDG_CONFIG_HOME/jouet/rules`) | always | warn |
| A repository's | `.jouet/rules/*.toml` at the repository root | once trusted with `jouet rule trust` | deny |

A file can hold any number of `[[rule]]` tables, and every rule has a `name` of its own.

Rules of yours and a repository's **only add decisions**. jouet enforces this, so a rule written in a hurry, by you or an agent, cannot weaken the safety it already has:

- **No replacing.** A rule with the name of a built-in rule is refused (`jouet rule list` shows it as broken), and the built-in rule keeps running.
- **No overruling.** Rules run built-in rules first, and the first denial wins. A script's `jouet.allow()` decides nothing for any other rule.
- **No locking out.** A denial of a command that only runs `jouet` becomes a warning, so a bad rule never stops you running `jouet rule list` or `jouet setup --uninstall` to fix it.
- **The sandbox.** It holds for every script (see below).

A repository's rules can deny commands and put text in front of the agent, so they only run once you trust them. `jouet rule trust` records a hash of every rule file and script in `.jouet/rules`. Adding or changing one makes the directory untrusted again, and `jouet rule list` says so. `jouet rule untrust` withdraws trust.

To turn rules off without deleting them, list them in `~/.config/jouet/config.toml`, or in `JOUET_RULES_DISABLE` (comma-separated):

```toml
[rules]
disable = ["git-stash", "no-verify"] # "all" turns every rule off
```

## Writing a rule

A rule says what it is for, which calls it matches, and what the agent is told. Write its examples first. They are the tool calls the rule must fire on, and the near misses it must leave alone, after [ast-grep](https://ast-grep.github.io/guide/test-rule.html)'s valid and invalid rule tests. They pin down what the rule is for more exactly than its `why`, and jouet holds every rule to them before it can block anything:

```toml
#:schema https://jouet.jdx.dev/schema/rules.json
[[rule]]
name = "no-rm-rf"
why = "rm -r deletes for good, and agents are quick to reach for it"
mode = "warn"
author = "jdx"
created = "2026-10-30"
message = "rm -r deletes for good; is this path right?"

[rule.match]
command = "rm"
flags = ["-r", "-R", "--recursive"]

[rule.examples]
# Calls the rule must fire on.
fires = ["rm -rf target", "sudo rm -r build"]
# Near misses it must leave alone.
quiet = ["rm file.txt", "echo 'rm -rf /'", "git commit -m 'rm -rf old code'"]
```

The `#:schema` line lets editors that read it ([Tombi](https://tombi-toml.github.io/tombi/), [Taplo](https://taplo.tamasfe.dev/)) complete and check the file. `jouet rule schema` prints the same JSON schema.

A rule's keys:

| Key | Meaning |
|---|---|
| `name` | Required. Lowercase words joined by dashes, unique among the rules jouet loads. |
| `why` | Required. What the rule is for, in one line. |
| `mode` | `warn` or `deny`. A `warn` rule's denials are shown to the agent as warnings and the command runs. A rule of yours defaults to `warn`, so a new rule can prove itself before it blocks anything. |
| `scope` | Optional `builtin`, `user` or `repo`; must match where the file lives. |
| `author` | Who wrote it: a person, or the agent session that did. |
| `created` | When. |
| `message` | What the agent is told when the rule matches. |
| `suggest` | A command to run instead. The agent sees ``Run `SUGGEST` instead.`` before the message. |
| `needs` | The jouet command the advice names, such as `["wait", "ci"]`. The rule stays off until the installed jouet has it. |
| `requires` | The project files the rule is about: `requires = { files = ["pitchfork.toml", ".config/pitchfork.toml"] }`. The rule runs only where one of them is in the call's directory or one above it, so a script about them never starts a VM elsewhere. Its examples run as if one were. |
| `match` | Which calls it matches: a table, or a list of `[[rule.match]]` tables of which any may match. |
| `examples` | `fires` and `quiet`, the calls it must fire on and leave alone. |
| `script` | A Luau file beside this one, for logic a matcher cannot express. See [Scripts](#scripts). |
| `on` | `pre-tool-use` (the default), or `post-tool-use` for a script that records checks. |

### Matching a command

A matcher's keys must all hold. A list in a key matches if any of its items does, so `command = ["npm test", "pnpm test"]` matches either. Every key but `tool`, `paths`, `unless_paths`, `input`, `unless_input`, `seen`, `min_read_lines`, `fnox` and `secret_files` is about a command, and needs `command`.

| Key | Matches |
|---|---|
| `command` | The program and its subcommands, such as `"git stash pop"`. Options before the first subcommand are skipped (`git -C dir add`, `cargo +nightly test`). `*` and `?` are wildcards: `"python3.*"`. |
| `flags` | The command has one of these options. `-f` matches inside a cluster such as `-fu`, and `--force` matches `--force=x`. |
| `unless_flags` | It has none of these. |
| `exact_flags` | Its options are exactly these, whatever their values. |
| `value_flags` | Its options that take a value, so that in `git commit -m -n` the `-n` is the message, and `-mn` is `-m n`. |
| `args` | One of its arguments (the words after its subcommands that are not options or their values) matches one of these wildcard patterns: `args = [".", ":/"]`, or `"+?*"` for a `+` refspec. |
| `unless_args` | None of its arguments matches. |
| `max_args` | It has at most this many arguments: `max_args = 0` for a bare `git stash pop`. |
| `background` | It runs in the background: `&` follows it, or the harness runs the whole call in the background (Claude Code's `run_in_background`). `background = true` for `npm run dev &`. |
| `text` | Regular expressions that must each match one of its words after the program, or one of its here-documents. `\|` gives alternatives within one. |
| `with` | Another command in the same call matches this too, as in `sleep 60 && gh pr checks`. It takes the command keys. |
| `unless_with` | No other command in the same call matches this, as in `rg TODO \| head`. It takes the command keys. |
| `via` | The wrapper it ran through, such as `rtk`, `sudo` or `mise`. |
| `tool` | The harness's tool name, such as `Edit`. Without it, a matcher with `command` matches any tool that runs a command. |
| `paths` | For a file tool, the path it reads or writes. A pattern without a `/` matches the file name; one with a `/` matches the path from the working directory, where `*` stays within a directory and `**` crosses them. |
| `input` | The tool's input has these fields with these values, such as `input = { output_mode = "content" }` for Claude Code's Grep. A value that is not a string matches as its JSON text; `*` and `?` are wildcards. |
| `unless_input` | The tool's input has none of these fields, such as `head_limit`. |
| `seen` | Whether the session was already shown everything the call reads, every line, in files unchanged since. The hook works it out from what the session read (Claude Code's Read tool, shell calls made only of `cat`, `head`, `tail`, `sed -n` and `nl`) and what `jouet search` showed it. Any other call is unseen. |
| `min_read_lines` | The call puts at least this many lines of files in the context: Claude Code's Read (up to 2,000 lines without a `limit`), or a shell read with `cat`, `head`, `tail`, `sed -n` or `nl`. The hook counts them. |
| `unless_paths` | For a file tool, the path matches none of these patterns, read as in `paths`. |
| `prints` | Whether the command's standard output reaches the transcript: it is not redirected to a file (`>`, `>>`, `&>`), piped into another command, or captured with `$(...)`. `>&2` still reaches it. |
| `fnox` | Whether the call runs in a project that uses [fnox](https://fnox.jdx.dev): a `fnox.toml`, `.fnox.toml` or `.config/fnox.toml` in the working directory or above it. |
| `secret_vars` | The command expands, outside single quotes, a variable the project's fnox config defines as a secret: `$NAME` or `${NAME}`. |
| `secret_args` | One of its arguments is the name of a secret the project's fnox config defines, as in `printenv NAME`. |
| `secret_files` | The call reads a file a fnox `plain` provider reads secrets from: a file tool's path, or with `command`, one of its arguments, compared as paths from the working directory. |

For example, the built-in `force-push` rule:

```toml
[[rule.match]]
command = "git push"
flags = ["-f", "--force"]
unless_flags = ["--force-with-lease"]
# `-o` takes a value, so `-of` is a push option, not a force.
value_flags = ["-o", "--push-option"]

# A `+` refspec forces that one ref.
[[rule.match]]
command = "git push"
args = "+?*"
unless_flags = ["--force-with-lease"]
```

The command is read the way a shell reads it, so a rule never has to pick apart quoting itself:

- **Quoted text, here-document bodies and comments are data.** In `git commit -m "never git add -A"`, the only command is `git commit`.
- **Commands are split at `;`, `&&`, `||`, `|` and newlines.** Leading reserved words such as `do` and `then` are dropped, along with variable assignments and redirections.
- **Commands inside `$(...)` and in scripts passed to `sh -c` or `bash -lc` match too.**
- **Wrappers are looked through:** `env FOO=1`, `sudo`, `timeout 60`, `nohup`, `rtk` and `mise x tool --` are skipped, `via` records them, and the program is the basename of what runs. A wrapper with nothing to run, such as a bare `env`, is the program itself.

`setup` sends Claude Code's hooks shell commands and its Read and Grep tools, and Codex's shell commands only. A `tool` or `paths` matcher for another tool fires under `jouet rule test`, and in a hook for whatever other tools a harness is set up to send.

### Examples

Every `fires` example must make the rule deny, warn, or record a check. Every `quiet` example must make it say nothing and record nothing. Examples check what the rule decides before the mode applies, so a warn-mode matcher fires with `deny`.

An example is a shell command, or a table for a call that needs more than that:

```toml
fires = [
  "git stash",
  { command = "git stash pop", harness = "codex" },
  { tool = "Edit", path = "Cargo.lock", action = "deny" },
]
```

| Key | Meaning |
|---|---|
| `command` | The shell command. Required for a `Bash` call. |
| `tool` | The harness's tool name; `Bash` if omitted. |
| `path` | For a file tool, the path it reads or writes. |
| `harness` | `claude`, `codex` or another harness; `test` if omitted. |
| `exit_code` | For a post-tool-use rule; `0` if omitted. |
| `action` | For a `fires` example: `deny` or `warn`, when it matters which. |
| `kind` | For a `fires` example of a post-tool-use rule: the kind it must record, `test`, `lint` or `build`. |
| `scope` | For a `fires` example with a `kind`: the scope the record must have, `full`, or `partial` and its selector. |
| `input` | The tool's input: `{ tool = "Grep", input = { pattern = "TODO", output_mode = "content" } }`, or `{ command = "npm run dev", input = { run_in_background = true } }` for a `Bash` call. |
| `seen` | Whether the session already saw what the call reads, as the hook would work it out; `false` if omitted. |
| `read_lines` | How many lines of files the call reads, as the hook would count them. |
| `fnox` | The call runs in a fnox project whose config defines these names: `fnox = {}`, or `fnox = { secrets = ["STRIPE_KEY"], files = ["secrets.env"] }`. Omitted, it does not. |
| `git` | What `jouet.git.state()` reports for a script: `git = { dirty = false }` for a work tree with nothing uncommitted, `true` for one with changes. Omitted, the probe has no answer, as every other probe in an example. |

Every rule needs at least one example of each kind:

- **Built-in rules** are held to theirs by jouet's own test suite.
- **A rule of yours or a repository's** without them still runs, but only warns, whatever its `mode` says. `jouet rule list` notes why.
- **Promotion and sharing.** `jouet rule promote` and `jouet share` refuse a rule until it passes examples of both kinds.
- **Drafted rules.** [Self-winding](/winding) writes both kinds into every rule it drafts, and drops a draft that fails its own.

### Testing a rule

`jouet rule test` runs a rule's examples. It fails if one does not do what it expects, showing what was expected and what the rule did, or if the rule lacks either kind. Examples run without [probes](#probes), so they give the same answer on any machine: each probe returns `nil` and a reason. Say you add `rm -ri build` as a near miss, since `-i` asks before each file:

```output
$ jouet rule test ./no-rm-rf.toml
ok    fires         rm -rf target
ok    fires         sudo rm -r build
ok    quiet         rm file.txt
ok    quiet         echo 'rm -rf /'
ok    quiet         git commit -m 'rm -rf old code'
FAIL  quiet         rm -ri build
      - expected: quiet
      + actual:   deny: rm -r deletes for good; is this path right?
no-rm-rf: 5 of 6 examples passed
```

`unless_flags = ["-i", "--interactive"]` fixes it. A file of several rules runs them all. A file that does not load says where:

```output
$ jouet rule test ./no-rm-rf.toml
jouet: no-rm-rf does not load: ./no-rm-rf.toml:12: unknown field `flag`, expected one of `tool`, `paths`, `command`, `via`, `flags`, `unless_flags`, `exact_flags`, `value_flags`, `args`, `unless_args`, `max_args`, `background`, `text`, `with`
```

`jouet rule test --all` runs every rule's examples: built in, yours, and this repository's, trusted or not.

```output
$ jouet rule test --all
ok    sleep-poll         12 examples passed
ok    review-threads     7 examples passed
ok    python-edit        7 examples passed
ok    git-add-all        14 examples passed
ok    git-stash          25 examples passed
ok    no-verify          14 examples passed
ok    force-push         12 examples passed
ok    print-secrets      44 examples passed
ok    secret-files       25 examples passed
ok    push-untested      13 examples passed
ok    scoped-lint        21 examples passed
ok    affected-tests     22 examples passed
ok    pitchfork-start    8 examples passed
ok    pitchfork-stop     9 examples passed
ok    pitchfork-logs     7 examples passed
ok    job-start          26 examples passed
ok    job-ready          12 examples passed
ok    job-logs           11 examples passed
ok    job-stop           15 examples passed
ok    claimed-path       16 examples passed
ok    checks             66 examples passed
ok    reread             12 examples passed
ok    large-read         11 examples passed
ok    unbudgeted-search  23 examples passed
FAIL  no-rm-rf           it has no `quiet` examples: near misses it must leave alone
24 of 25 rules passed their examples
```

It also runs a rule against one command or a recorded hook payload. The result is then what jouet would enforce, after the mode:

```sh
jouet rule test git-stash --command 'git stash pop'
jouet rule test checks --command 'cargo test' --exit-code 101
jouet rule test no-rm-rf --payload hook-input.json
```

`--replay` runs it over every call in the hook ledger instead. It lists the calls the rule fires on and the near misses it stays quiet on (calls running the same programs), and fails if it fires on nothing:

```console
$ jouet rule test no-tags --replay
no-tags: fired on 8 of 28 calls in the ledger
fires on:
     6x  warn         git tag <arg>
     2x  warn         git tag -l
stays quiet on (near misses):
     2x  allow        git status
```

`jouet rule new NAME --why "..."` writes `~/.config/jouet/rules/NAME.toml` from a template, in warn mode, with a matcher to fill in and empty `fires` and `quiet` lists to fill in first. `jouet rule promote NAME` switches it to deny once it passes them. [Self-winding](/winding) drafts rules like this from the ledger on its own.

## Scripts

Most rules are a command shape and a message, and a matcher says that in a few lines. When a rule needs logic, such as comparing numbers, reading a task name or asking jouet's state, it names a Luau script beside its file. Four built-in rules do: `checks` reads `mise run test:e2e` and `npm run lint` by their task names, and records runs rather than denying them; `push-untested` matches a push, then asks the ledger and git whether a full test run passed at this tree; `affected-tests` matches a whole-suite test run, then asks whether jouet can select tests here (`jouet.test.adapters()`) and whether the last test run was a whole suite too; `claimed-path` matches an edit or a commit, then asks `jouet.claims.held()` whether another session claims a path it touches.

```toml
[[rule]]
name = "long-sleep"
why = "a long sleep spends the session's time doing nothing"
needs = ["wait"]
script = "long-sleep.luau"
# The script only runs on calls this matches.
match = { command = "sleep", args = "*" }
examples = { fires = ["sleep 90"], quiet = ["sleep 1", "echo sleep 90"] }
```

```lua
--!strict
local rule: JouetRule = {
	check = function(call: JouetCall): JouetDecision?
		for _, command in call.commands do
			local seconds = tonumber(command.argv[2])
			if command.program == "sleep" and seconds and seconds >= 10 then
				return jouet.warn("Wait for what you need with `jouet wait` instead of sleeping.")
			end
		end
		return nil
	end,
}

return rule
```

The script returns a table with a `check` function, which gets the tool call and returns `jouet.deny(message)`, `jouet.warn(message)`, `jouet.allow()`, or `nil` to say nothing. The rule's name, mode, `needs`, `on` and examples stay in its TOML, and a script rule takes no `message` or `suggest`: its `check` says what to tell the agent. `jouet rule new NAME --script` writes both files.

A matcher in front of a script is worth having: a hook only starts a Luau VM when a script has a call to look at, so a rule that matches `sleep` costs nothing on every other command. Rules without a script never start one.

`call.commands` is the command parsed the way a shell parses it, as for a matcher. Each command has its `words`, its `argv` without wrappers, its `program`, its `heredocs`, the wrappers it ran `via`, and whether it is `nested`. The `jouet` module has helpers for the common cases:

- `jouet.git(command)` returns git's subcommand and arguments, past global options such as `-C dir`. (`jouet.git` is also where the `jouet.git.state()` probe lives.)
- `jouet.gh(command)` returns gh's arguments.
- `jouet.options(args)` returns the arguments before `--`.
- `jouet.short_flag(arg, "f", "o")` reads short-option clusters such as `-fu`.
- `jouet.has_command("wait", "ci")` asks whether the installed jouet has a command.

`jouet rule types` prints the whole API as Luau type definitions.

A rule with `on = "post-tool-use"` runs its script after a command finished. Each top-level command then has a `status`: the exit status it is known to have had. A failure belongs only to the last pipeline, so `cargo test; echo done` failing gives `cargo test` no status. Success belongs to every command in the final `&&` chain. `jouet.record("test", command, scope, selector)` records a run with that status. `scope` is `"full"` when the run covered the whole project and `"partial"` otherwise, and `selector` says what a partial run picked, such as `"-p jouet-edit"`. A rule that leaves the scope out records a partial run: only a rule that knows a run covered everything says so. The built-in `checks` rule uses it so jouet can refuse to let an agent finish while a test, lint or build run failed, or after only partial test runs on changed code ([how](/setup#finishing-on-a-failing-check)). Its `fires` examples can name the scope they expect, as in `{ command = "cargo test -p a", kind = "test", scope = "partial -p a" }`.

### Type checking

Scripts can be type-checked with [luau-lsp](https://github.com/JohnnyMorganz/luau-lsp) against the definitions jouet prints:

```sh
jouet rule types > jouet.d.luau
luau-lsp analyze --definitions=jouet.d.luau ~/.config/jouet/rules/*.luau
```

### Luau rule files

Before rules were TOML, a rule was a whole `.luau` file with a `--- why:` header and its `on`, `needs` and `examples` in the table it returns. Those files still load, but are deprecated and stop loading in jouet 0.1.0; `jouet rule list` names each one. To convert one, write its header and examples as a `[[rule]]` table in a `.toml` file of the same name, and either express its `check` as a matcher or keep the file as the rule's `script`, without the header and the table's `on`, `needs` and `examples`.

## Probes

A script can also ask about jouet's state. The probes only read, and each returns its value, or `nil` and the reason it has none:

| Probe | Returns |
|---|---|
| `jouet.pr.status(pr?)` | What [`jouet pr status`](/commands/pr-status) reports for the PR, or for this branch's PR |
| `jouet.pr.next(pr?)` | What [`jouet pr next`](/commands/pr-next) says the PR needs next |
| `jouet.ci.last_run(kind?, scope?)` | The newest `test`, `lint` or `build` run the hooks recorded in their ledger, of any scope or only `"full"` or `"partial"` ones: its `kind`, `ok`, `scope`, the `tree` it ran against, `exit_code`, command `shape` and time. `nil` when there is no ledger yet |
| `jouet.git.state()` | The branch, its upstream, `ahead` and `behind`, and whether the work tree is `dirty`, from plain `git` |
| `jouet.git.tree()` | A hash of the content of every tracked file in the work tree, staged or not: the `tree` a recorded run carries. A commit with the same content hashes the same |
| `jouet.git.changed(scope?)` | The files a commit would take, as absolute paths: what is staged (`"staged"`, the default), or every changed tracked file (`"tracked"`, what `git commit -a` takes) |
| `jouet.claims.held(paths)` | The live [claims](/commands/claim) of any session that cover the paths, each with the `path` it covers (relative to the repository's root), its `pattern`, `session`, `worktree`, `branch`, `note` and `expires_at`; compare `session` with `call.session` |
| `jouet.cargo.workspace(dir?)` | The Cargo workspace around a directory: its `root`, the `package` a plain cargo command there builds, every member's name in `members`, and whether it sets `default_members` |
| `jouet.pitchfork.daemons()` | The daemons the project's pitchfork config declares, from `pitchfork.toml`, `pitchfork.local.toml` and their `.config/` forms from the filesystem root down to the call's directory, a nearer file winning: each one's `name`, its `run` command as one line, the `ports` it is said to listen on (`port`, `expected_port`, `ready_port`, and `--port N` or `-p N` in `run`), and its `file`. Read from the files, without running pitchfork; `nil` when none declares any |
| `jouet.repo()` | The work tree's `root`, whether it is a linked worktree, and its GitHub `owner`, `name` and `repo` |
| `jouet.env(name)` | An environment variable on a short allowlist, such as `CI` or `JOUET_*`. A name with `TOKEN`, `SECRET`, `PASSWORD` or `KEY` in it is never on it. |

```lua
check = function(call)
	local state = jouet.git.state()
	if state and state.behind and state.behind > 0 then
		return jouet.warn(`{state.branch} is {state.behind} commits behind {state.upstream}`)
	end
	return nil
end,
```

Every probe is cached for the rest of the hook call, so asking twice costs nothing, and runs inside the rule's budget. A probe gets whatever time the rule has left, and one that cannot answer in time fails open like the rule. `git` probes run without taking git's optional locks, so they never get in the way of the agent's own git.

**The GitHub probes are off in hooks.** A hook runs before every tool call, and a round trip to GitHub would slow every one of them, so `jouet.pr.status` and `jouet.pr.next` return `nil` and say so. To turn them on, set this in `~/.config/jouet/config.toml`, or set `JOUET_RULES_NETWORK=true`:

```toml
[rules]
network = true
```

A probe still has to answer within the rule's 50 ms. [Routines](/routines) reach GitHub through their own primitives instead.

The hooks record which work tree each run was in, by a hash of its root rather than its path, so `jouet.ci.last_run` passes over runs from other repositories.

## The sandbox and its limits

Each script runs in a sandboxed Luau VM:

- **What a rule can reach.** The standard library, without `require`, plus the `jouet` module and its read-only [probes](#probes). Running jouet's commands and writing are for [routines](/routines), not rules. Luau has no `io`, and its `os` is limited to the clock. The `jouet` module and the libraries are read-only, and every rule has globals of its own.
- **Budgets.** Every call to a script's `check`, and every loading of its file, has a 50 ms time budget and a budget of a million interrupts. Luau raises an interrupt at every function call and loop iteration. The whole VM may use 32 MiB.
- **Failing open.** A rule that does not load, or a script that does not compile, throws, runs out of budget or returns something unexpected, says nothing. jouet reports it and carries on with the other rules.
- **Compiled-script cache.** Compiled scripts are cached in `$XDG_CACHE_HOME/jouet/luau`, keyed by a hash of their source. Luau does not verify bytecode, so that directory must stay writable only by you.

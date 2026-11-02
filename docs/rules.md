---
description: Rules that read the command an agent is about to run, or has just run, and deny it, warn about it, or record it as a check.
---

# Rules

A jouet rule reads a tool call, such as the shell command an agent is about to run, and decides what happens to it: jouet denies the call with one line of advice, lets it run with a warning, or says nothing. jouet ships its own rules, for instance one that denies `git add -A` and one that denies a bare `git stash`, and you can add your own in the same format. Write one when an agent keeps making a mistake you can recognise from the shape of a command.

A rule is a `[[rule]]` table in a TOML file: which calls it matches, what it tells the agent, and examples of calls it must fire on and near misses it must leave alone. This one warns before a recursive `rm`:

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

Once [`jouet setup`](/setup) has installed the hooks, a rule saved as `~/.config/jouet/rules/no-rm-rf.toml` applies from the agent's next tool call. When the agent runs `rm -rf build`, the command still runs, because the rule is in `warn` mode, and Claude Code passes the agent this:

```text
rm -r deletes for good; is this path right? [jouet rule: no-rm-rf]
```

`jouet rule test ./no-rm-rf.toml` checks the rule against its examples ([Testing a rule](#testing-a-rule)), and `jouet rule promote no-rm-rf` switches it to `deny` once they pass. A rule that needs logic a matcher cannot express hands the decision to a small [Luau script](#scripts).

::: warning Early development
The rule format is new and may change between releases. `jouet rule schema` always describes the installed version.
:::

## Built-in rules

jouet's own rules are written in the same format as yours. `jouet rule list` shows every rule jouet loads:

```output
$ jouet rule list
sleep-poll          enabled  deny    builtin  builtin  2026-10-30  A sleep loop around a CI query spends a model turn on every check; `jouet wait ci` blocks until the checks settle.
ci-rerun            enabled  deny    builtin  builtin  2026-10-31  Rerunning CI blind wastes a run on a real failure; `jouet ci explain` says whether the failure is flaky, infrastructure or real, and reruns only the first two.
scoped-lint         enabled  warn    builtin  builtin  2026-10-30  While iterating, linting the whole project re-reports every file and costs minutes and pages of output; checking only the changed files through hk gives their findings as file:line:rule.
…
```

The columns are name, state, mode, scope, author, created, and why. The state is `enabled`, `disabled`, `needs` and the jouet command the rule is waiting for, `untrusted`, or `broken` and the error. The mode is `deny`, `warn`, or `record` for a rule that records checks after a call. The scope says where the rule comes from ([below](#where-rules-come-from)).

The built-in rules, grouped by what they are about:

| About | Rules |
|---|---|
| CI and pull requests | `sleep-poll`, `ci-rerun`, `review-threads` |
| Editing files | `python-edit`, `sed-line-edit` |
| git | `git-add-all`, `git-stash`, `no-verify`, `force-push`, `reset-to-remote`, `rebase-merged-base` |
| Secrets | `print-secrets`, `secret-files`, `committed-secrets` |
| Tests and lints | `scoped-lint`, `affected-tests`, `piped-test-output`, and `checks`, which denies nothing and records test, lint and build runs for the [stop check](/setup#finishing-on-a-failing-check) |
| Servers and background work | `job-start`, `job-ready`, `job-logs`, `job-stop`, `pitchfork-start`, `pitchfork-stop`, `pitchfork-logs` |
| What goes into the context | `reread`, `large-read`, `unbudgeted-search`, `unbounded-query`, `curl-json` |
| Sessions and trust | `claimed-path`, `trust` |

[Steering rules](/setup#steering-rules) in the setup guide describes what they deny and what they suggest instead. Their source, in [`crates/jouet-rules/rules`](https://github.com/jdx/jouet/tree/main/crates/jouet-rules/rules), is a good place to copy from when you write your own.

## Where rules come from

| Source | Directory | Runs | Mode unless the rule says |
|---|---|---|---|
| Built in | embedded in the binary | always | deny |
| [Enforced](/plugins#enforced-rules-and-routines) | `/etc/jouet/rules/*.toml` (`$JOUET_SYSTEM_DIR`), and the system's plugins | always, unless its group or others can write it; your settings cannot turn them off | deny |
| Yours | `~/.config/jouet/rules/*.toml` (`$XDG_CONFIG_HOME/jouet/rules`) | always | warn |
| [A plugin](/plugins#plugins) you pin | its `rules/`, named `PLUGIN/NAME` | once fetched as pinned | deny |
| A repository's | `.jouet/rules/*.toml` at the repository root, and its plugins | once [trusted](/plugins#trusting-a-repository) with `jouet trust` | deny |

A file can hold any number of `[[rule]]` tables, and every rule has a `name` of its own. Rules load in the order of the table, and a name already taken is refused, so neither your rules nor a repository's can take the place of an enforced one.

For a rule from outside, the scope column of `jouet rule list` says whether its source runs: `repo:trusted`, `repo:changed` or `repo:untrusted`; `plugin:NAME`, or `plugin:NAME:mismatch`; and `enforced`, or `enforced:writable` when its group or others can write the system directory. `--json` gives the same as `trust` and `plugin` fields. [Plugins and trust](/plugins) covers where rules from outside come from, how they are pinned and trusted, and what that does not protect against.

Rules of yours and a repository's **only add decisions**. jouet enforces this, so a rule written in a hurry, by you or an agent, cannot weaken the safety jouet already gives:

- **No replacing.** A rule with the name of a built-in rule is refused (`jouet rule list` shows it as broken), and the built-in rule keeps running.
- **No overruling.** Built-in rules run first, and the first denial wins. A script's `jouet.allow()` decides nothing for any other rule.
- **No locking out.** A denial of a command that only runs `jouet` becomes a warning, so a bad rule never stops you running `jouet rule list` or `jouet setup --uninstall` to fix it.
- **Examples first.** A rule from outside runs only once it passes its examples of both kinds.
- **The sandbox.** It holds for every script ([The sandbox and its limits](#the-sandbox-and-its-limits)).

### Turning rules off

To turn rules off without deleting them, list them in `~/.config/jouet/config.toml`, or in `JOUET_RULES_DISABLE` (comma-separated). Neither turns off an [enforced](/plugins#enforced-rules-and-routines) rule:

```toml
[rules]
disable = ["git-stash", "no-verify"] # "all" turns every rule off
```

## Writing a rule

1. `jouet rule new NAME --why "..."` writes `~/.config/jouet/rules/NAME.toml` from a template, in warn mode, with a matcher to fill in and empty `fires` and `quiet` lists. `--script` also writes `NAME.luau` for a [script](#scripts). `--author` says who is writing it (by default `$JOUET_SESSION_ID`, or the harness's session id), and `--from-note ID` starts from a rule [self-winding](/winding) drafted.
2. Fill in the examples first: the calls the rule must fire on, and the near misses it must leave alone.
3. Fill in the matcher and the message.
4. Run `jouet rule test NAME` until every example passes, and `jouet rule test NAME --replay` to see what the rule would have done to the calls jouet's hooks have already seen.
5. `jouet rule promote NAME` switches it from warn to deny once it passes its examples. `--reason` records why, and `jouet rule log` shows every change made to your rules. Only rules in `~/.config/jouet/rules` can be promoted: built-in rules are part of jouet, and a repository's change through a reviewed PR.

Writing the examples first follows [ast-grep](https://ast-grep.github.io/guide/test-rule.html)'s valid and invalid rule tests. They say what the rule is for more exactly than its `why` does, and jouet holds every rule to them before it can block anything.

The `#:schema` line at the top of the file lets editors that read it ([Tombi](https://tombi-toml.github.io/tombi/), [Taplo](https://taplo.tamasfe.dev/)) complete and check the file. `jouet rule schema` prints the same JSON schema.

When a rule cannot express what you need, `jouet rule gap "TEXT"` records the wish, and `--rule NAME` the rule you were writing, for `jouet share --idea`. jouet records one on its own when a script reads a `jouet` field that does not exist.

### Rule keys

| Key | Meaning |
|---|---|
| `name` | Required. Lowercase words joined by dashes, starting with a letter, up to 64 characters, unique among the rules jouet loads. |
| `why` | Required. What the rule is for, in one line. |
| `mode` | `warn` or `deny`. A `warn` rule's denials are shown to the agent as warnings and the command runs. A rule of yours defaults to `warn`, so a new rule can prove itself before it blocks anything; a rule from any other source defaults to `deny`. |
| `scope` | Optional `builtin`, `user`, `repo`, `plugin` or `enforced`; it must match where the file lives. A plugin's or the system directory's file may also say `repo`. |
| `author` | Who wrote it: a person, or the agent session that did. |
| `created` | When, as a date. |
| `message` | What the agent is told when the rule matches. Required unless a `script` decides, and a script rule leaves it out. |
| `suggest` | A command to run instead. The agent sees ``Run `SUGGEST` instead.`` before the message. For a script rule, only the session-start primer names it. |
| `needs` | The jouet command the advice names, such as `["wait", "ci"]`. The rule stays off until the installed jouet has it. |
| `requires` | The project files the rule is about: `requires = { files = ["pitchfork.toml", ".config/pitchfork.toml"] }`. The rule runs only where one of them is in the call's directory or one above it, so a script about them never starts a VM elsewhere. Its examples run as if one were. |
| `match` | Which calls it matches: a table, or a list of `[[rule.match]]` tables of which any may match. |
| `examples` | `fires` and `quiet`, the calls it must fire on and leave alone. |
| `script` | A Luau file beside this one, or below its directory, for logic a matcher cannot express. See [Scripts](#scripts). |
| `on` | `pre-tool-use` (the default), or `post-tool-use` for a script that records checks. Only a `script` rule can run after a tool. |
| `primer` | `false` keeps a `deny` rule out of the session-start primer, which names the deny rules so the agent avoids the calls they deny; `true` if omitted. For a rule that denies on what a call would change, not on how it looks, such as `committed-secrets`, naming it saves no call. |

### Matching a command

A matcher's keys must all hold. A list in a key matches if any of its items does, so `command = ["npm test", "pnpm test"]` matches either; `text` is the exception, where every pattern must match. Every key but `tool`, `paths`, `unless_paths`, `input`, `unless_input`, `seen`, `min_read_lines`, `min_code_lines`, `min_tracked_files`, `fnox` and `secret_files` is about a command, and needs `command`. A matcher with none of `command`, `tool` and `paths` would match every call, and is refused.

| Key | Matches |
|---|---|
| `command` | The program and its subcommands, such as `"git stash pop"`. Options before the first subcommand are skipped (`git -C dir add`, `cargo +nightly test`). `*` and `?` are wildcards: `"python3.*"`. |
| `flags` | The command has one of these options. `-f` matches inside a cluster such as `-fu`, and `--force` matches `--force=x`. Write each as `-x` or `--long`, without a value; a cluster such as `-rf` is refused. |
| `unless_flags` | It has none of these. |
| `exact_flags` | Its options are exactly these, whatever their values. |
| `value_flags` | Its options that take a value, so that in `git commit -m -n` the `-n` is the message, and `-mn` is `-m n`. |
| `args` | One of its arguments (the words after its subcommands that are not options or their values) matches one of these wildcard patterns: `args = [".", ":/"]`, or `"+?*"` for a `+` refspec. |
| `unless_args` | None of its arguments matches. |
| `max_args` | It has at most this many arguments: `max_args = 0` for a bare `git stash pop`. |
| `background` | It runs in the background: `&` follows it, or the harness runs the whole call in the background (Claude Code's `run_in_background`). `background = true` for `npm run dev &`. |
| `text` | Regular expressions that must each match one of its words after the program, or one of its here-documents. `\|` gives alternatives within one. |
| `with` | Another command in the same call matches this too, as in `sleep 60 && gh pr checks`. It takes the command keys, not `tool`, `paths` or another `with`. |
| `unless_with` | No other command in the same call matches this, as in `rg TODO \| head`. It takes the command keys, like `with`. |
| `via` | The wrapper it ran through, such as `rtk`, `sudo` or `mise`. |
| `tool` | The harness's tool name, such as `Edit`; `*` and `?` are wildcards. Without it, a matcher with `command` matches any tool that runs a command. |
| `paths` | For a file tool, the path it reads or writes. A pattern without a `/` matches the file name; one with a `/` matches the path from the working directory, where `*` stays within a directory and `**` crosses them. |
| `unless_paths` | For a file tool, the path matches none of these patterns, read as in `paths`. |
| `input` | The tool's input has these fields with these values, such as `input = { output_mode = "content" }` for Claude Code's Grep. A value that is not a string matches as its JSON text; `*` and `?` are wildcards. |
| `unless_input` | The tool's input has none of these fields, such as `head_limit`. |
| `seen` | Whether the session was already shown everything the call reads, every line, in files unchanged since. The hook works it out from what the session read (Claude Code's Read tool, shell calls made only of `cat`, `head`, `tail`, `sed -n` and `nl`) and what `jouet search` showed it. Any other call is unseen. |
| `min_read_lines` | The call puts at least this many lines of files in the context: Claude Code's Read (up to 2,000 lines without a `limit`), or a shell read with `cat`, `head`, `tail`, `sed -n` or `nl`. The hook counts them. |
| `min_code_lines` | Like `min_read_lines`, counting only source files `jouet read --outline` can outline, so a long log, a command's saved output or a document read whole does not count. |
| `min_tracked_files` | The work tree tracks at least this many files (`git ls-files`), so a search of all of it can print a lot. jouet's hook counts them only for the Grep tool and commands that run grep or rg. Outside a repository the count is unknown, and the key holds. |
| `prints` | Whether the command's standard output reaches the transcript: it is not redirected to a file (`>`, `>>`, `&>`), piped into another command, or captured with `$(...)`. `>&2` still reaches it. |
| `fnox` | Whether the call runs in a project that uses [fnox](https://fnox.jdx.dev): a `fnox.toml`, `.fnox.toml` or `.config/fnox.toml` in the working directory or above it. |
| `secret_vars` | The command expands, outside single quotes, a variable the project's fnox config defines as a secret: `$NAME` or `${NAME}`. |
| `secret_args` | One of its arguments is the name of a secret the project's fnox config defines, as in `printenv NAME`. |
| `secret_files` | The call reads a file a fnox `plain` provider reads secrets from: a file tool's path, or with `command`, one of its arguments, compared as paths from the working directory. |

For example, the built-in `force-push` rule has two matchers, either of which fires it:

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
value_flags = ["-o", "--push-option"]
```

jouet reads the command the way a shell reads it, so a rule never has to pick apart quoting itself:

- Quoted text, here-document bodies and comments are data. In `git commit -m "never git add -A"`, the only command is `git commit`.
- Commands are split at `;`, `&&`, `||`, `|` and newlines. Leading reserved words such as `do` and `then` are dropped, along with variable assignments and redirections.
- Commands inside `$(...)` and in scripts passed to `sh -c` or `bash -lc` are matched too.
- Wrappers are looked through: `env FOO=1`, `sudo`, `timeout 60`, `nohup`, `rtk` and `mise x tool --` are skipped, `via` records them, and the program is the basename of what runs. A wrapper with nothing to run, such as a bare `env`, is the program itself.

Which tool calls reach the rules depends on the harness. `jouet setup` sends Claude Code's shell commands, its Read and Grep tools and its file edits (Edit, Write, MultiEdit and NotebookEdit), but only Codex's shell commands; [What the hooks do](/setup#the-hooks) has the details. A `tool` or `paths` matcher for another tool fires under `jouet rule test`, and in a hook for whatever other tools a harness is set up to send.

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
| `code_lines` | How many of those lines are in source files `jouet read --outline` can outline. |
| `tracked_files` | How many files the work tree tracks. Omitted, the count is unknown, as outside a repository. |
| `fnox` | The call runs in a fnox project whose config defines these names: `fnox = {}`, or `fnox = { secrets = ["STRIPE_KEY"], files = ["secrets.env"] }`. Omitted, it does not. |
| `session` | The agent session making the call, as the hook payload's `session_id`. Omitted, it has none. |
| `git` | What the git probes report for a script: `dirty` and `branch` for `jouet.git.state()`, and `ahead` for `jouet.git.ahead()`, whatever branch it asks about. `git = { dirty = false }` is a work tree with nothing uncommitted and `true` one with changes; `dirty` is `false` if omitted, and without `branch`, HEAD is detached. Omitted, these probes have no answer. |
| `claims` | The claims `jouet.claims.held()` finds, each with its `pattern` and `session`, and optionally `worktree` (`/elsewhere` if omitted), `branch` and `note`: `claims = [{ pattern = "src/cmd/", session = "theirs" }]`. |
| `stack` | Where `jouet.stack.layer()` places the current branch: `stack = { branch = "feat/cache", parent = "feat/parser", parent_merged = true }`, with `trunk` (`main` if omitted). |
| `secrets` | What `jouet.secrets.scan()` finds, for any scope: `secrets = [{ rule = "github-pat", path = "src/config.rs", line = 3 }]`, with a `commit` for an outgoing scan. |

Examples run without [probes](#probes), so they give the same answer on any machine: each probe returns `nil` and a reason, unless the example answers it with `git`, `claims`, `stack` or `secrets`.

Every rule needs at least one example of each kind:

- **Built-in rules** are held to theirs by jouet's own test suite.
- **A rule of yours** without them still runs, but only warns, whatever its `mode` says. `jouet rule list` notes why:

  ```output
  note: no-rm-rf only warns, and cannot be promoted, until it has examples of both kinds: it has no `quiet` examples: near misses it must leave alone; see `jouet rule test no-rm-rf`
  ```

- **A rule from outside** (a repository's, a plugin's or the system's) that lacks either kind, or fails one, does not run at all, and `jouet rule list` shows it as broken. `jouet trust` and `jouet plugin add` refuse it.
- **Promotion and sharing.** `jouet rule promote` and `jouet share` refuse a rule until it passes examples of both kinds.
- **Drafted rules.** [Self-winding](/winding) writes both kinds into every rule it drafts, and drops a draft that fails its own.

### Testing a rule

`jouet rule test` runs a rule's examples. It fails if one does not do what it expects, showing what was expected and what the rule did, or if the rule lacks either kind. Say you add `rm -ri build` to the `quiet` list of the rule at the top of this page, since `-i` asks before each file:

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

Adding `unless_flags = ["-i", "--interactive"]` to the matcher fixes it. A file of several rules runs them all. A file that does not load says where:

```output
$ jouet rule test ./no-rm-rf.toml
jouet: no-rm-rf does not load: ./no-rm-rf.toml:12: unknown field `flag`, expected one of `tool`, `paths`, `command`, `via`, `flags`, `unless_flags`, `exact_flags`, `value_flags`, `args`, `unless_args`, `max_args`, `background`, `text`, `with`, `unless_with`, `input`, `unless_input`, `seen`, `min_read_lines`, `min_code_lines`, `min_tracked_files`, `unless_paths`, `fnox`, `secret_files`, `secret_vars`, `secret_args`, `prints`
```

`jouet rule test --all` runs every rule's examples: built in, the system's, yours, the plugins', and this repository's, trusted or not.

```output
$ jouet rule test --all
ok    sleep-poll          12 examples passed
ok    ci-rerun            16 examples passed
…
FAIL  no-rm-rf            it has no `quiet` examples: near misses it must leave alone
32 of 33 rules passed their examples
```

It also runs a rule against one command, or against a recorded Claude Code or Codex hook payload (`-` reads it from stdin). The result is then what jouet would enforce, after the mode, so a warn-mode rule says `warn`:

```sh
jouet rule test git-stash --command 'git stash pop'
jouet rule test checks --command 'cargo test' --exit-code 101
jouet rule test no-rm-rf --payload hook-input.json
```

`--replay` runs the rule over every call in the hook ledger instead. It lists the calls the rule fires on and the near misses it stays quiet on (calls running the same programs), and fails if it fires on nothing:

```console
$ jouet rule test no-tags --replay
no-tags: fired on 8 of 28 calls in the ledger
fires on:
     6x  warn         git tag <arg>
     2x  warn         git tag -l
stays quiet on (near misses):
     2x  allow        git status
```

## Scripts

Most rules are a command shape and a message, and a matcher says that in a few lines. When a rule needs logic, such as comparing numbers, reading a task name or asking about jouet's state, it names a Luau script beside its file:

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

The script returns a table with a `check` function, which gets the tool call and returns `jouet.deny(message)`, `jouet.warn(message)`, `jouet.allow()`, or `nil` to say nothing. In a warn-mode rule, `jouet.deny` warns. The rule's name, mode, `needs`, `on` and examples stay in its TOML, and a script rule takes no `message`: its `check` says what to tell the agent. Its `suggest`, if any, is for the session-start primer. `jouet rule new NAME --script` writes both files.

A matcher in front of a script is worth having: a hook only starts a Luau VM when a script has a call to look at, so a rule that matches `sleep` costs nothing on every other command. Rules without a script never start one.

These built-in rules have scripts: `affected-tests`, `checks`, `claimed-path`, `committed-secrets`, `job-start`, `piped-test-output`, `pitchfork-start`, `pitchfork-stop`, `python-edit`, `rebase-merged-base`, `reset-to-remote`, `scoped-lint`, `sed-line-edit` and `unbounded-query`. Some of what they do:

- `checks` reads `mise run test:e2e` and `npm run lint` by their task names, and records runs rather than denying them.
- `affected-tests` matches a whole-suite test run, then asks whether jouet can select tests here (`jouet.test.adapters()`) and whether the last test run was a whole suite too.
- `piped-test-output` matches a test or build runner in a call that also runs `head`, `tail` or `grep`, then follows the pipeline, and the file the output went to, to see whether it is that run's output being cut. A cut `jouet test` is told to drop the cut instead, since its output is already short and a pipe hides its exit status.
- `reset-to-remote` asks `jouet.git.ahead()` how many local commits a `git checkout -B` would drop.
- `rebase-merged-base` asks `jouet.stack.layer()` whether a rebased branch's prerequisite has merged.
- `claimed-path` matches an edit or a commit, then asks `jouet.claims.held()` whether another session claims a path it touches.
- `committed-secrets` matches a commit or a push, then asks `jouet.secrets.scan()` what gitleaks' rules find in the lines it would add.

### The call

`check` gets the call as a table:

| Field | Holds |
|---|---|
| `event` | `"pre-tool-use"` or `"post-tool-use"`. |
| `harness` | `"claude"`, `"codex"`, or `"test"` under `jouet rule test` unless an example names a harness. |
| `tool` | The harness's tool name, such as `"Bash"`. |
| `command` | The shell command, for a shell tool. |
| `path` | The file a file tool reads or writes. |
| `cwd` | The call's working directory. |
| `session` | The agent session making the call: the hook payload's `session_id`. |
| `exit_code` | The command's exit code, after a tool (post-tool-use only). |
| `commands` | Every command the shell command runs: top-level ones first, then those inside `$(...)` and `sh -c '...'`. |

`call.commands` is the command parsed the way a shell parses it, as for a matcher. Each command has:

| Field | Holds |
|---|---|
| `words` | Every word, with leading reserved words (`do`, `then`, ...), variable assignments and redirections removed. |
| `argv` | `words` without wrappers such as `env FOO=1`, `sudo`, `timeout 60`, `rtk` and `mise x tool --`, so `argv[1]` is the program that runs. |
| `program` | `argv[1]`'s basename: `/usr/bin/git` and `git.exe` are both `git`. |
| `heredocs` | The bodies of its here-documents. |
| `via` | The wrappers `argv` looked through, by basename, outermost first: `rtk git status` ran via `{ "rtk" }`. |
| `op` | The operator after it: `"seq"`, `"and"`, `"or"`, `"pipe"` or `"background"`; `nil` at the end of the script. |
| `stdout_to` | The file its standard output is redirected to (`> out.log`, `&> all`), as written; `nil` when there is none. |
| `env` | The variable assignments before the program (`RUST_LOG=1 cargo test`), which `words` leaves out. |
| `nested` | Whether it is inside `$(...)` or a script passed to `sh -c`. |
| `matched` | Whether one of the rule's matchers matched it: the reason the script runs. Always false for a rule without matchers, and in `jouet.parse`. |
| `status` | After a tool, the exit status it is known to have had ([Recording checks](#recording-checks)). |
| `text` | `argv` joined with spaces, cut to 80 characters, for messages. |
| `line` | `argv` joined with spaces, whole. |

### Helpers

The `jouet` module has helpers for the common cases:

- `jouet.git(command)` returns git's subcommand and arguments, past global options such as `-C dir`, or `nil` and `{}` when the command is not git. (`jouet.git` is also where the `jouet.git.state()` probe lives.)
- `jouet.gh(command)` returns gh's arguments, or `nil` when the command is not gh.
- `jouet.options(args)` returns the arguments before `--`.
- `jouet.short_flag(arg, "f", "o")` reads short-option clusters such as `-fu`: whether `arg` holds `-f`, stopping at the first letter that takes a value (here `o`).
- `jouet.has_command("wait", "ci")` asks whether the installed jouet has a command.
- `jouet.unwrap(words)` returns the words without wrappers, as `argv` has them, and the wrappers it looked through: for a command named inside another, such as the one after `jouet test --cmd --`.
- `jouet.dirs(commands)` says where each command runs after the top-level `cd`s before it, by its index: `nil` in the call's own directory, a path (relative, absolute, or under `~` or `$HOME`), or `false` after a `cd` it cannot follow (`cd`, `cd -`, `cd $DIR`). The probes and `jouet.record` take that path as `dir`.
- `jouet.parse(script, exit_code?)` parses a script the way `call.commands` is built; with an exit code, top-level commands get a `status`.
- `jouet.contains(list, value)` and `jouet.basename(path)`.

`jouet rule types` prints the whole API as Luau type definitions.

### Recording checks

A rule with `on = "post-tool-use"` runs its script after a command finished. Each top-level command then has a `status`: the exit status it is known to have had. A failure belongs only to the last pipeline, so `cargo test; echo done` failing gives `cargo test` no status. Success belongs to every command in the final `&&` chain, except one piped into another without `set -o pipefail`: `cargo test | tail` exits with tail's status, so `cargo test` gets none.

`jouet.record("test", command, scope, selector, dir)` records a run with that status; it does nothing when `status` is `nil`, and a kind other than `test`, `lint` or `build` is an error. `scope` is `"full"` when the run covered the whole project and `"partial"` otherwise, and `selector` says what a partial run picked, such as `"-p jouet-edit"`. `dir` is where a `cd` before the command moved it (relative, absolute, or under `~` or `$HOME`), so the run counts for that work tree. A rule that leaves the scope out records a partial run: only a rule that knows a run covered everything says so.

The built-in `checks` rule uses it so jouet can refuse to let an agent finish while a test, lint or build run failed, or after it changed files with no test run passing since ([how](/setup#finishing-on-a-failing-check)). Its `fires` examples can name the scope they expect, as in `{ command = "cargo test -p a", kind = "test", scope = "partial -p a" }`.

### Type checking

Scripts can be type-checked with [luau-lsp](https://github.com/JohnnyMorganz/luau-lsp) against the definitions jouet prints:

```sh
jouet rule types > jouet.d.luau
luau-lsp analyze --definitions=jouet.d.luau ~/.config/jouet/rules/*.luau
```

### Luau rule files

Before rules were TOML, a rule was a whole `.luau` file with a `--- why:` header and its `on`, `needs` and `examples` in the table it returns. Those files still load, but are deprecated and stop loading in jouet 0.1.0; `jouet rule list` names each one. To convert one, write its header and examples as a `[[rule]]` table in a `.toml` file of the same name, and either express its `check` as a matcher or keep the file as the rule's `script`, without the header and the table's `on`, `needs` and `examples`.

## Probes

A script can also ask about jouet's state and the work tree. The probes only read. Each returns its value, or `nil` and the reason it has none:

| Probe | Returns |
|---|---|
| `jouet.pr.status(pr?)` | What [`jouet pr status`](/commands/pr-status) reports for the PR `pr` names (a number, URL, `OWNER/REPO#N` or branch), or for this branch's PR, without waiting for an unknown mergeable state to settle |
| `jouet.pr.next(pr?)` | What [`jouet pr next`](/commands/pr-next) says the PR needs next |
| `jouet.ci.last_run(kind?, scope?, dir?)` | The newest `test`, `lint` or `build` run the hooks recorded in their ledger, of any scope or only `"full"` or `"partial"` ones: its `kind`, the `rule` that recorded it, `ok`, `scope`, the `tree` it ran against, `exit_code`, command `shape`, time (`ts`, `duration_ms`) and `harness`. With `dir`, the runs of the work tree there. `nil` when there is no ledger yet or no such run |
| `jouet.git.state(dir?)` | The `branch` (absent when HEAD is `detached`), `head` (its first 12 digits), its `upstream`, `ahead` and `behind`, whether the work tree is `dirty` (anything changed, untracked or conflicted), and how many files are `changed`, `untracked` and `conflicted`, from `git status`. With `dir`, of the work tree there |
| `jouet.git.tree(dir?)` | A hash of the content of every tracked file in the work tree, staged or not, and of untracked files that are not ignored, unless there are more than 1,000 of those: the `tree` a recorded run carries. A commit with the same content hashes the same |
| `jouet.git.changed(scope?)` | The files a commit would take, as absolute paths: what is staged (`"staged"`, the default), or every changed tracked file (`"tracked"`, what `git commit -a` takes) |
| `jouet.git.ahead(branch, of, dir?)` | How many commits on the local `branch` are not in `of` (a branch, `origin/…` or a SHA): what moving the branch to `of` would drop; `nil` when there is no such local branch |
| `jouet.claims.held(paths)` | The live [claims](/commands/claim) of any session that cover the paths (relative to the call's directory, or absolute; a directory covers the claims inside it), each with the `path` it covers (relative to the repository's root), its `pattern`, `session`, `worktree`, `branch`, `note` and `expires_at`; compare `session` with `call.session`. Empty when none do; `nil` outside a repository |
| `jouet.stack.layer()` | The current branch's place in a gh-stack stack: its `branch`, `parent` (the layer below, or the trunk), `trunk`, and `parent_merged`, true when the layer below was submitted as a PR and its remote branch is gone; `nil` when gh-stack does not track the branch |
| `jouet.cargo.workspace(dir?)` | The Cargo workspace around a directory (the call's when omitted): its `root`, the `package` a plain cargo command there builds, every member's name in `members`, and whether it sets `default_members`; `nil` outside a Cargo project |
| `jouet.test.adapters()` | The test runners `jouet test --affected` can select for, in the nearest directory from the call's up to the work tree's root that has any, such as `{ "vitest" }`; empty when there are none |
| `jouet.pitchfork.daemons()` | The daemons the project's pitchfork config declares, from `pitchfork.toml`, `pitchfork.local.toml` and their `.config/` forms from the filesystem root down to the call's directory, a nearer file winning: each one's `name`, its `run` command as one line, the `ports` it is said to listen on (`port`, `expected_port`, `ready_port`, and `--port N` or `-p N` in `run`), and its `file`. Read from the files, without running pitchfork; `nil` when none declares any |
| `jouet.secrets.scan(scope?, paths?)` | What [gitleaks' rules](/commands/secrets), with the repository's `.gitleaks.toml` and `.gitleaksignore`, find in the lines a commit or push would add: `"staged"` (the default), `"tracked"` (what `git commit -a` takes) or `"outgoing"` (each commit not on any remote), and the `paths`, which a `git add` in the same call is about to stage. Each finding has its `rule`, `path`, `line`, `commit` for an outgoing scan, `redacted` match and `fingerprint`; never the secret. `nil` and the reason for a diff over 1 MiB, or a scan that runs out of budget |
| `jouet.repo()` | The work tree's `root`, whether it is a linked worktree, and its GitHub `owner`, `name` and `repo` |
| `jouet.file.contains(path, text)` | Whether the file at `path` (from the work tree's root when relative) holds `text` exactly: only `true` or `false`, never the file; `nil` and a reason when it cannot be read or is over 4 MiB |
| `jouet.env(name)` | An environment variable on a short allowlist: `AI_AGENT`, `CARGO_TARGET_DIR`, `CI`, `CLAUDECODE`, `CODEX_SANDBOX`, `GITHUB_ACTIONS`, `GITHUB_BASE_REF`, `GITHUB_HEAD_REF`, `GITHUB_REF_NAME`, `GITHUB_REPOSITORY`, `LANG`, `MISE_ENV`, `NODE_ENV`, `SHELL`, `TERM`, `USER`, `VIRTUAL_ENV` and `JOUET_*`. A name with `TOKEN`, `SECRET`, `PASSWORD` or `KEY` in it is never on it, and any name not on it is an error |

```lua
check = function(call)
	local state = jouet.git.state()
	if state and state.behind and state.behind > 0 then
		return jouet.warn(`{state.branch} is {state.behind} commits behind {state.upstream}`)
	end
	return nil
end,
```

Every probe is cached for the rest of the hook call, so asking twice costs nothing. The probes a check asks share 200 ms between them, apart from the rule's own 50 ms, so waiting on a `git` process that is slow to start on a busy machine does not run the rule out of time. A probe gets whatever is left of those 200 ms, and one that cannot answer in time returns `nil` and the reason, which the rule can act on or ignore. `git` probes run without taking git's optional locks, so they never get in the way of the agent's own git.

The hooks record which work tree each run was in, by a hash of its root rather than its path, so `jouet.ci.last_run` passes over runs from other repositories.

### GitHub probes in hooks

A hook runs before every tool call, and a round trip to GitHub would slow every one of them, so in hooks `jouet.pr.status` and `jouet.pr.next` return `nil` and say they are off. To turn them on, set this in `~/.config/jouet/config.toml`, or set `JOUET_RULES_NETWORK=true`:

```toml
[rules]
network = true
```

A probe still has to answer within the probes' 200 ms. [Routines](/routines) reach GitHub through their own primitives instead.

## The sandbox and its limits

Each script runs in a sandboxed Luau VM:

- **What a rule can reach.** The standard library, without `require`, plus the `jouet` module and its read-only [probes](#probes). Running jouet's commands and writing are for [routines](/routines), not rules. Luau has no `io`, and its `os` is limited to the clock. The `jouet` module and the libraries are read-only, and every rule has globals of its own.
- **Budgets.** Every call to a script's `check`, and every loading of its file, has a 50 ms time budget for its own work, 200 ms for the [probes](#probes) it waits on, and a budget of a million interrupts. Luau raises an interrupt at every function call and loop iteration. The whole VM may use 32 MiB.
- **Failing open.** A rule that does not load, or a script that does not compile, throws, runs out of budget or returns something unexpected, says nothing. jouet reports it and carries on with the other rules.
- **Compiled-script cache.** Compiled scripts are cached in `$XDG_CACHE_HOME/jouet/luau`, keyed by a hash of their source. Luau does not verify bytecode, so that directory must stay writable only by you.
- **Every source alike.** A script from a repository, a plugin or the system directory runs in the same sandbox and budgets as a built-in one.

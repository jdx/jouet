---
description: Routines are small Luau programs on jouet's primitives (GitHub, git, wait, edit, the ledger and mise), run by joue, with declared arguments and capabilities and recorded-response tests.
---

# Routines

A routine is a small [Luau](https://luau.org) program for a job an agent would otherwise do by hand, one command at a time: look something up, decide, then act or report. It is written on jouet's primitives (the authenticated GitHub client, git, the wait engine, all-or-nothing edits, the hooks' ledger and mise), declares what it may reach, and runs with `joue NAME`. You or your agent write one when the same job keeps being done step by step, such as finding which of your PRs need you, or landing a PR once CI passes.

This routine, saved as `~/.config/jouet/routines/triage.luau`, lists your open PRs that need you:

```lua
--- about: Say which of my open PRs need me, and why
--- capabilities: github.read
--- usage: flag "--repo <OWNER/NAME>" var=#true help="Only these repositories"
return {
	run = function(args)
		local found = github.prs({ mine = true, repo = args.repo, settle = 0 })
		local mine = joue.array({})
		for _, pr in found.prs do
			if pr.attention then
				print(`{pr.repo}#{pr.number}: {pr.attention[1].message}`)
				table.insert(mine, { pr = `{pr.repo}#{pr.number}`, attention = pr.attention[1].code })
			end
		end
		return { prs = mine }, if #mine > 0 then joue.exit.act else joue.exit.ok
	end,
}
```

Its header is enough for `joue` to describe it:

```console
$ joue triage --help
Say which of my open PRs need me, and why

Usage: joue triage [FLAGS]

Flags:
      --repo <OWNER/NAME>  Only these repositories
      --json               Print one JSON document instead of text
  -h, --help               Print help
```

`joue triage` asks GitHub for your open PRs in one batched query, as [`jouet pr status --mine`](/commands/pr-status) does, and prints a line for each one with an [attention code](/commands/pr-status#attention), such as `jdx/jouet#5: 2 unresolved review threads`. With `--json` it prints the table it returns instead, each PR with its code (`unresolved_threads`), and it exits 10 when any PR needs you. Those values come from the example in [Testing](#testing), which runs the routine against a recorded GitHub answer. [Writing a routine](#writing-a-routine) explains each part of the file.

## Running a routine

`joue` (French for "play!") is a second name for the jouet binary, so routines have a namespace of their own and `jouet` stays the core:

```text
joue NAME [ARGS...]        run a routine; the same as `jouet run NAME [ARGS...]`
joue NAME --help           what it takes, from its usage spec
joue --list                every routine, where it comes from and what it reaches
joue test [NAME]           run routines' examples against recorded responses
joue types                 the Luau type definitions of the primitives
```

```console
$ joue --list
land        Land a PR: ask pr-next, wait for CI if it is still running, then pr land
            reaches github.read, github.write
pr-next     Say the one thing each PR needs next, and the command for it
            reaches github.read
ship-check  Before saying done: run the project's tests, then check the PR, and report both
            reaches github.read, mise.run
sweep       List a PR's unresolved review threads as a compact work list, posting nothing
            reaches github.read
run one with `joue NAME`; `joue NAME --help` says what it takes
```

A routine from a file also shows the path it comes from. The two names point at each other: `joue pr status` says "did you mean `jouet pr status`?", and `jouet triage`, when `triage` is a routine, says to run `joue triage`.

`jouet completion SHELL --joue` prints the shell completion script for `joue`. It completes `joue`'s own flags and commands; routine names are not completed yet.

## The built-in routines

| Routine | What it does |
|---|---|
| `pr-next [PR...]` | The routine behind [`jouet pr next`](/commands/pr-next): the same arguments, output and exit codes. |
| `land [PR]` | Runs `pr-next`. If CI is still running, waits for it and asks again. Once the PR is `ready_to_merge`, runs `jouet pr land`, which still refuses whatever it refuses by hand, honouring branch protection. `--method` picks squash (the default), merge or rebase. Without `--yes` the land is only previewed. |
| `sweep [PR]` | Lists the unresolved review threads as one line each: ID (the first comment's, as `jouet pr threads` takes it), `path:line`, author, and the first line of the comment. It never posts. |
| `ship-check [PR]` | Runs the project's tests, then checks the PR, and reports both. It fails when the tests fail or the PR needs attention. Without a PR for the branch, it reports the tests alone. |

`pr next` became a routine because it costs nothing measurable: against a real PR the routine and the Rust version it replaced take the same time within noise, since nearly all of it is GitHub's. Commands where speed is the point, such as the hooks, `edit` and the wait engine, stay in Rust as primitives.

## Choosing a routine, a mise task or a rule

Routines are code and rules are configuration. A rule is a TOML matcher that steers the agent's tool calls, with a Luau script only as an escape hatch; a routine is a Luau program from the start, because composing primitives with conditions and loops is what it is for. Pick by what the job needs:

- **A mise task** for fixed commands with no decisions, specific to one project and useful to people too. It is the default: start with one.
- **A routine** once conditionals or parsing creep in: logic over structured state, built on jouet's primitives, with structured output, often reusable across projects.
- **A [rule](/rules)** to steer the agent's own tool calls, typically toward a task or a routine: a TOML matcher and a message, such as "use `joue triage` instead of that `gh` loop". A rule runs on every tool call, so it stays configuration; a routine runs when asked, so it can be a program.

[Self-winding](/winding) can propose any of the three, and the agent picks the kind that fits.

## Writing a routine

A routine is a `.luau` file. It starts with a header of `--- key: value` lines, which ends at the first line that does not start with `---`, and it returns a table with a `run` function. The header says what the routine is, what it takes and what it reaches:

| Key | Meaning |
|---|---|
| `about` | Required: one line, shown by `joue --list` and `--help`. |
| `help` | The longer description `--help` shows; repeat it for more lines. |
| `capabilities` | What it may reach, separated by commas or spaces. See [capabilities](#capabilities). |
| `usage` | Its args and flags, as a [usage](https://usage.jdx.dev/spec/) spec in KDL; repeat it for each. `--json` is added to every routine, and `--yes` and `--dry-run` to one that writes. |
| `name` | Optional; when given, it must be the file's name. |
| `author` | Optional, and not used by `joue`. |

Any other key, an unknown capability, a missing `about` or a usage spec that does not parse marks the routine broken: `joue --list` still lists it, with the reason, and running it exits 2 with the same reason. Nothing in the header needs the Luau VM, so listing routines and printing their help runs no script.

`run(args)` gets the parsed arguments by name, `-` written as `_`: a string for a flag that takes a value, `true` for a switch, a count for a repeatable switch, a list for a repeatable flag or a variadic argument, and nothing for what was not given. A misspelled flag is an error before the routine runs. In the example, `--repo` is repeatable (`var=#true`), so `args.repo` is a list, which is what `github.prs` takes.

What `run` returns is its result:

- **Text:** `print(...)` is the output in text mode. A string returned as the first value is added to that text.
- **JSON:** a table returned as the first value is the `--json` document. A table that should print as `[]` when empty is marked with `joue.array(t)`, and `joue.object(t, keys)` fixes the order of an object's keys; other objects print with their keys sorted. A routine that returns no table prints `{"output": TEXT}` under `--json`, an error prints `{"error": {code, message, hint}}`, and writes that were only previewed are listed in the document as `planned`.
- **Exit code:** the second return value, 0 to 255, usually from `joue.exit`: `ok`, `failed`, `usage`, `not_found`, `precondition`, `remote`, `timeout`, and `pr next`'s `act` and `waiting`. `joue.fail(code, message, hint)` stops with an error, printed the way jouet's commands print theirs.

The `joue` module has the rest:

| Field | What it is |
|---|---|
| `joue.json` | Whether `--json` was given |
| `joue.acting` | Whether `--yes` was given, so writes are made |
| `joue.dry_run` | Whether `--dry-run` was given |
| `joue.exit` | The exit codes above, by name |
| `joue.fail(code, message, hint)` | Stops the routine with an error |
| `joue.array(t)`, `joue.object(t, keys)` | Mark a table as a JSON list, or as an object with its keys in this order |
| `joue.duration(text)` | A duration such as `90s` or `1h30m` in seconds, or `nil` and the error |
| `joue.now()` | The time, in Unix seconds |
| `joue.call(name, args)` | Runs another routine with these words and returns its result and exit code, as `land` does with `pr-next` |

What a routine started with `joue.call` prints is dropped. It runs with its own capabilities, which the caller must hold too, or the call fails with exit `4`; calls nest at most eight deep.

## Testing

A routine's table can carry `examples`: runs against recorded responses, and what each must come to. `joue test NAME` runs them; `joue test` runs every routine's. A routine without examples fails `joue test`, and one that writes refuses to run at all without them (see [Writes need `--yes`](#writes-need-yes)).

This example, added after `run` in the table the triage routine returns, checks its text, its JSON and its exit code against a recorded answer from GitHub:

```lua
	examples = {
		{
			name = "PRs with unresolved threads are listed, and the run exits 10",
			args = { "--repo", "jdx/jouet" },
			fixture = "fixtures/triage.json",
			exit = 10,
			output = { "jdx/jouet#5: 2 unresolved review threads", "jdx/jouet#3: 3 unresolved review threads" },
			json = { prs = { { pr = "jdx/jouet#5", attention = "unresolved_threads" }, { pr = "jdx/jouet#3" } } },
		},
	},
```

```console
$ joue test triage
ok   triage: PRs with unresolved threads are listed, and the run exits 10
1 routine, 1 example, all passed
```

| Field | Meaning |
|---|---|
| `name` | What the example shows, printed by `joue test` |
| `args` | The words after the routine's name; leave it out for none. `yes = true` adds `--yes` |
| `fixture` | A JSON file beside the routine, or the same table inline |
| `exit` | The exit code (default `0`) |
| `output` | Text the output must contain: a string or a list. An error shows as `jouet: ...` |
| `json` | Fields the `--json` document must have, run separately. A list must have as many entries |
| `planned` | Writes that must have been previewed (`{}` for none) |
| `requests` | Requests that must have been made, as text found in `METHOD PATH BODY` or `jouet COMMAND` |

A fixture holds the answers. The triage example's, `fixtures/triage.json` beside the routine, holds one: a recorded GitHub reply to the search for your PRs. A fixture with an answer of every kind looks like this:

```json
{
  "github": [
    { "method": "POST", "path": "/graphql", "match": "\"n0\":7", "body": { "data": { "l0": { "pullRequest": {} } } } }
  ],
  "jouet": [{ "command": "wait ci", "exit_code": 0, "json": { "result": "met" } }],
  "git": { "state": { "branch": "main" }, "repo": { "repo": "jdx/demo" } },
  "mise": { "test": { "ok": true, "exit_code": 0, "command": ["cargo", "test"] } },
  "ledger": []
}
```

`github` is served by a local fake GitHub: each request gets the first unused entry with its method and path whose `match` text is in the body, then the last matching one again, so a poll can repeat. An entry answers with its `status`, `body` and `headers`. `jouet` answers jouet's commands in order, by the start of their command line. `git`, `mise` and `ledger` answer those primitives by name. Nothing reaches the real GitHub, git or mise, and edits are only ever previewed, so `joue test` is safe to run anywhere. A request with no recorded answer fails the example and says which.

## Primitives

`joue types` prints their Luau type definitions, for [luau-lsp](https://github.com/JohnnyMorganz/luau-lsp) or `luau-analyze --definitions`.

| Primitive | What it does | Capability |
|---|---|---|
| `github.prs{prs, mine, search, repo, settle}` | The PRs `pr status` selects, with their state, checks, review, threads and [attention codes](/commands/pr-status#attention), in one batched query. `settle` is the seconds to wait for GitHub to work out an unknown mergeable state (20 unless told otherwise, as with `pr status --settle`) | `github.read` |
| `github.graphql(query, vars)` | A GraphQL query; GitHub's `NOT_FOUND` paths are `nil` | `github.read` |
| `github.get(path, {etag})` | One REST read: `{ status, etag, next, body }`. With `etag`, an unchanged answer is a free `304` | `github.read` |
| `github.paginate(path, {max_pages})` | A REST list, following `Link` headers and joining the pages (10 pages unless told otherwise) | `github.read` |
| `github.repo()` | The repository the git remote names | `github.read` |
| `github.mutate(query, vars)`, `github.request(method, path, body)` | A GraphQL mutation, and a REST `POST`, `PUT`, `PATCH` or `DELETE` | `github.write` |
| `git.state()`, `git.diff{base, staged, stat, paths}`, `git.log{range, max, paths}`, `git.refs(pattern)`, `git.rev(name)` | The work tree: branch, upstream, ahead and behind, and uncommitted changes; diffs; history, newest first (20 commits unless told otherwise); and refs. Read-only | `git.read` |
| `wait.ci`, `wait.pr`, `wait.run` | [`jouet wait`](/commands/wait)'s engine for CI, PRs and workflow runs | `github.read` |
| `wait.file`, `wait.log` | Waiting on files and log lines | `fs.read` |
| `wait.port`, `wait.http` | Waiting on ports and URLs | `net` |
| `wait.pid` | Waiting for a process to exit | none |
| `jouet.pr_status`, `jouet.pr_next`, `jouet.pr_threads` | jouet's own PR commands | `github.read` |
| `jouet.pr_reply`, `jouet.pr_resolve`, `jouet.pr_land` | jouet's commands that post or merge, with every refusal they have by hand | `github.write` |
| `jouet.diag_check{files, changed, max}`, `jouet.diag_delta{...}` | [`jouet diag`](/commands/diag): the project's hk checks on files, as findings, or only those an edit introduced | `fs.read` |
| `jouet.code.symbols{query}`, `jouet.code.callers{symbol}`, `jouet.code.callees{symbol}`, `jouet.code.impact{symbol}`, `jouet.code.explore{query}`, `jouet.code.affected{files}` | [`jouet graph`](/commands/graph)'s queries, refreshing the graph before each one; all but `affected` take `budget` and `page` to page a long answer | `fs.read` |
| `jouet.docs.lookup{pkg, symbol, lang, budget, private}` | [`jouet docs`](/commands/docs): a dependency's outline, or one definition's signature, doc and source, at the version the project locked, from its installed sources | `fs.read` |
| `jouet.deps.info{packages, refresh}`, `jouet.deps.audit{ecosystem, refresh}` | [`jouet deps`](/commands/deps): whether packages exist, their latest version, licence and advisories, and the locked dependencies checked against OSV | `net` |
| `jouet.db.schema{table, url, remote, budget}`, `jouet.db.query{sql, url, remote, limit, width, budget, timeout}` | [`jouet db`](/commands/db): a SQLite or Postgres database's tables, columns, keys and indexes, and one read-only query's rows, cut to a limit | `db.read` |
| `jouet.secrets.scan{paths, staged, diff}` | [`jouet secrets scan`](/commands/secrets): gitleaks' rules over files, the staged lines or the commits since a ref, with each finding's rule, file, line and redacted match; exit 1 when it finds any | `git.read` |
| `jouet.http.request{method, url, header, json_body, data, select, expect, budget, headers, no_follow, timeout}` | [`jouet http`](/commands/http): one request of any method, its status, headers and the JSONPaths selected, with hurl-style assertions; credentials redacted | `net` |
| `jouet.diag_fix{files, changed, step}` | The project's hk fixers on files (`hk run fix --safe`, which refuses a fixer whose effect is unknown or destructive) | `fs.write` |
| `edit.plan(spec, opts)` | Applies a [`jouet edit`](/commands/edit) spec in memory and reports each edit. `opts` takes `format` (`blocks`, `patch` or `json`), `diff` and `expect_hash`, the SHA-256 each path must have | `fs.read` |
| `edit.apply(spec, opts)` | Writes it only if every edit succeeded, all or nothing | `fs.write` |
| `ledger.entries{event, since, limit, all_repos}`, `ledger.last_run(kind)` | What the hooks recorded in this work tree, newest first, from the last 1 MiB of the ledger: command shapes, exit codes, rule decisions. `since` is a duration such as `2h`, `limit` defaults to 100, and `all_repos` reads every work tree's entries. `last_run` is the newest test, lint or build run, as `{ kind, ok, exit_code, shape, ts }`, or `nil` | `ledger.read` |
| `mise.tasks()`, `mise.config()`, `mise.tools()`, `mise.env()` | mise's task list and resolved config: `mise tasks ls`, `mise config ls`, `mise ls --current` and `mise env`, as JSON | `mise.read` |
| `mise.run(task, {args})`, `mise.test()` | Runs a mise task, or the project's test command, with the output in a log | `mise.run` |

GitHub goes through jouet's client, which finds a token that works (`JOUET_GITHUB_TOKEN`, `GH_TOKEN`, `GITHUB_TOKEN`, then `gh auth token`), retries 5xx answers and secondary rate limits, and says when an exhausted rate limit resets. A GitHub failure stops the routine with exit `5`.

The `wait` and `jouet` functions run the command as `jouet --json ...` and return `{ ok, exit_code, json, error, command }` rather than failing, since a wait that times out is an answer too. They take a table: positional arguments in order or by name, flags by name with `_` for `-` (`{ "123", log_lines = 20 }`). `wait cmd` and `edit --check`, which run shell commands, are not among them.

`mise.test()` runs the first test command the project has: a mise `test` task, a `package.json` `test` script, `Cargo.toml`, a justfile or Makefile `test` target, or `go.mod`. It and `mise.run` return `{ ok, exit_code, command, duration_ms, log, tail }`, where `log` is the file the output went to and `tail` its last 30 lines.

## Capabilities

A routine reaches only what its header declares:

| Capability | Reaches |
|---|---|
| `github.read` | GitHub queries and REST reads, `github.prs`, jouet's PR commands, and waits on CI, PRs and runs |
| `github.write` | GitHub mutations and REST writes, and jouet's commands that post or merge |
| `git.read` | The work tree's state, diffs, history and refs, and secret scans |
| `fs.read` | Planning edits, waits on files and logs, the project's hk checks, the code graph and dependencies' docs |
| `fs.write` | Writing edits, and running the project's hk fixers |
| `net` | Waits on ports and URLs, HTTP requests of any method (so `net` lets a routine `POST`), and package lookups |
| `ledger.read` | The hooks' ledger |
| `mise.read` | mise's config, tools, environment and task list |
| `mise.run` | Running a mise task, or the project's tests |
| `db.read` | A database's schema and read-only queries, local unless the call passes `remote` |

A primitive outside them fails with exit `4`, and the error names the line to add, so reading a routine's header is enough to know what running it can do. A routine named `branch` that calls `git.state()` without declaring `git.read` stops with:

```text
jouet: git.state needs the `git.read` capability, which branch does not declare
hint: add it to the header: `--- capabilities: git.read`
```

### Writes need `--yes`

A routine with a capability that writes (`github.write` or `fs.write`) gets `--yes` and `--dry-run`, and acts only with `--yes`. Without it, every write is previewed instead of made: a GitHub write returns `{ dry_run = true }`, an edit returns its plan, and `jouet pr land`, the thread commands and `jouet.diag_fix` run with `--dry-run`. The run then lists what it would have done and exits `4`, so an agent that forgot `--yes` gets a preview and a clear refusal, never a surprise. `--dry-run` previews the same way and exits as the routine says.

Such a routine must also carry [examples](#testing): without them it refuses to run.

### The sandbox

Routines run in a sandboxed Luau VM: the standard library without `require`, `io` or most of `os`, plus the primitives. The Luau between two primitive calls has a budget of 10 seconds and fifty million interrupts, in 256 MiB; a primitive's own time, such as a long `wait.ci`, does not count.

## Where routines come from

| Source | Directory | Runs |
|---|---|---|
| Built in | embedded in the binary | always |
| [Enforced](/plugins#enforced-rules-and-routines) | `/etc/jouet/routines/*.luau` (`$JOUET_SYSTEM_DIR`), and the system's plugins | always, unless its group or others can write it |
| Yours | `~/.config/jouet/routines/*.luau` (`$XDG_CONFIG_HOME/jouet/routines`) | always |
| [A plugin](/plugins#plugins) you pin | its `routines/`, run as `joue PLUGIN/NAME` | once fetched as pinned |
| A repository's | `.jouet/routines/*.luau` at the repository root, and its plugins | once [trusted](/plugins#trusting-a-repository) with `jouet trust`, with its rules |

A routine's name is its file name without `.luau`, in lowercase words joined by dashes; a plugin's routines are named after it, as `acme/deploy`. `help`, `test` and `types` are `joue`'s own commands, so no routine can have those names. As with [rules](/rules#where-rules-come-from), a routine cannot take a built-in one's name. A repository's routines are trusted together with its rules, by a hash of their files, so adding or changing one needs trusting again; until then they are listed but not run. A routine from outside runs in the same sandbox and with the same capabilities checks as yours. [Plugins and trust](/plugins) covers the rest.

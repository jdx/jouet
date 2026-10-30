---
description: Routines are small Luau programs on jouet's primitives (GitHub, git, wait, edit, the ledger and mise), run by joue, with declared arguments and capabilities and recorded-response tests.
---

# Routines

A routine is a small [Luau](https://luau.org) program written on jouet's primitives: the authenticated GitHub client, git, the wait engine, all-or-nothing edits, the hooks' ledger and mise. You or your agent write one when a job keeps being done step by step, such as "find my PRs that need me, most urgent first", and run it with `joue`.

`joue` (French for "play!") is a second name for the jouet binary, so routines have a namespace of their own and `jouet` stays the core:

```text
joue NAME [ARGS...]        run a routine; the same as `jouet run NAME [ARGS...]`
joue NAME --help           what it takes, from its usage spec
joue --list                every routine, where it comes from and what it reaches
joue test [NAME]           run routines' examples against recorded responses
joue types                 the Luau type definitions of the primitives
joue --trust | --untrust   trust this repository's .jouet/routines
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
```

`jouet completion SHELL --joue` prints the shell completion script for `joue`; it completes `joue`'s own flags and commands, and routine names are not completed yet.

The two names point at each other: `joue pr status` says "did you mean `jouet pr status`?", and `jouet triage`, when `triage` is a routine, says to run `joue triage`.

## The built-in routines

| Routine | What it does |
|---|---|
| `pr-next [PR...]` | The routine behind [`jouet pr next`](/commands/pr-next): the same arguments, output and exit codes. |
| `land [PR]` | Runs `pr-next`. If CI is still running, waits for it and asks again. Once the PR is `ready_to_merge`, runs `jouet pr land`. Without `--yes` the land is only previewed. |
| `sweep [PR]` | Lists the unresolved review threads as one line each: ID, `path:line`, author, and the first line of the comment. It never posts. |
| `ship-check [PR]` | Runs the project's tests, then checks the PR, and reports both. It fails when the tests fail or the PR needs attention. |

`pr next` became a routine because it costs nothing measurable: against a real PR the routine and the Rust version it replaced take the same time within noise, since nearly all of it is GitHub's. Commands where speed is the point, such as the hooks, `edit` and the wait engine, stay in Rust as primitives.

## Writing a routine

A routine is a `.luau` file. Its header says what it is, what it takes and what it reaches, and it returns a table with a `run` function:

```lua
--- about: Say which of my open PRs need me, most urgent first
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

The header lines:

| Key | Meaning |
|---|---|
| `about` | Required: one line, shown by `joue --list` and `--help`. |
| `help` | The longer description `--help` shows; repeat it for more lines. |
| `capabilities` | What it may reach, comma-separated. See [capabilities](#capabilities). |
| `usage` | Its args and flags, as a [usage](https://usage.jdx.dev/spec/) spec in KDL; repeat it for each. `--json` is added to every routine, and `--yes` and `--dry-run` to one that writes. |
| `name` | Optional; when given, it must be the file's name. |

`run(args)` gets the parsed arguments by name, `-` written as `_`: a string for a flag that takes a value, `true` for a switch, a list for a repeatable one or a variadic argument, and nothing for what was not given. A misspelled flag is an error before the routine runs.

What it returns is its result:

- **Text:** `print(...)` is the output in text mode.
- **JSON:** the first return value is the `--json` document. A table that should print as `[]` when empty is marked with `joue.array(t)`, and `joue.object(t, keys)` fixes the order of an object's keys; other objects print with their keys sorted.
- **Exit code:** the second return value, from `joue.exit`: `ok`, `failed`, `usage`, `not_found`, `precondition`, `remote`, `timeout`, and `pr next`'s `act` and `waiting`. `joue.fail(code, message, hint)` stops with an error, printed the way jouet's commands print theirs.

`joue.json` says whether `--json` was given, `joue.acting` whether `--yes` was, and `joue.call(name, args)` runs another routine and returns its result and exit code, as `land` does with `pr-next`.

## Where routines come from

| Source | Directory | Runs |
|---|---|---|
| Built in | embedded in the binary | always |
| Yours | `~/.config/jouet/routines/*.luau` (`$XDG_CONFIG_HOME/jouet/routines`) | always |
| A repository's | `.jouet/routines/*.luau` at the repository root | once trusted with `joue --trust` |

A routine's name is its file name without `.luau`, in lowercase words joined by dashes. As with [rules](/rules#where-rules-come-from), a routine cannot take a built-in one's name. A repository's routines are trusted by a hash of their files, so adding or changing one needs trusting again.

## Primitives

`joue types` prints their Luau type definitions, for [luau-lsp](https://github.com/JohnnyMorganz/luau-lsp) or `luau-analyze --definitions`.

| Primitive | What it does | Capability |
|---|---|---|
| `github.prs{prs, mine, search, repo, settle}` | The PRs `pr status` selects, with their state, checks, review, threads and [attention codes](/commands/pr-status#attention), in one batched query | `github.read` |
| `github.graphql(query, vars)` | A GraphQL query; GitHub's `NOT_FOUND` paths are `nil` | `github.read` |
| `github.get(path, {etag})` | One REST read: `{ status, etag, next, body }`. With `etag`, an unchanged answer is a free `304` | `github.read` |
| `github.paginate(path, {max_pages})` | A REST list, following `Link` headers (10 pages unless told otherwise) | `github.read` |
| `github.repo()` | The repository the git remote names | `github.read` |
| `github.mutate(query, vars)`, `github.request(method, path, body)` | A GraphQL mutation, and a REST `POST`, `PUT`, `PATCH` or `DELETE` | `github.write` |
| `git.state()`, `git.diff{base, staged, stat, paths}`, `git.log{range, max, paths}`, `git.refs(pattern)`, `git.rev(name)` | The work tree: branch and upstream, diffs, history and refs. Read-only | `git.read` |
| `wait.ci`, `wait.pr`, `wait.run` | [`jouet wait`](/commands/wait)'s engine for CI, PRs and workflow runs | `github.read` |
| `wait.file`, `wait.log` | Waiting on files and log lines | `fs.read` |
| `wait.port`, `wait.http` | Waiting on ports and URLs | `net` |
| `wait.pid` | Waiting for a process to exit | none |
| `jouet.pr_status`, `jouet.pr_next`, `jouet.pr_threads` | jouet's own PR commands | `github.read` |
| `jouet.pr_reply`, `jouet.pr_resolve`, `jouet.pr_land` | jouet's commands that post or merge, with every refusal they have by hand | `github.write` |
| `edit.plan(spec, opts)` | Applies a [`jouet edit`](/commands/edit) spec in memory and reports each edit | `fs.read` |
| `edit.apply(spec, opts)` | Writes it only if every edit succeeded, all or nothing | `fs.write` |
| `ledger.entries{event, since, limit}`, `ledger.last_run(kind)` | What the hooks recorded in this work tree: command shapes, exit codes, rule decisions | `ledger.read` |
| `mise.tasks()`, `mise.config()`, `mise.tools()`, `mise.env()` | mise's task list and resolved config | `mise.read` |
| `mise.run(task, {args})`, `mise.test()` | Runs a mise task, or the project's test command, with the output in a log | `mise.run` |

GitHub goes through jouet's client, which finds a token that works (`JOUET_GITHUB_TOKEN`, `GH_TOKEN`, `GITHUB_TOKEN`, then `gh auth token`), retries 5xx answers and secondary rate limits, and says when an exhausted rate limit resets. A GitHub failure stops the routine with exit `5`. The `wait` and `jouet` functions run the command as `jouet --json ...` and return `{ ok, exit_code, json, error, command }` rather than failing, since a wait that times out is an answer too. They take a table: positional arguments in order or by name, flags by name with `_` for `-` (`{ "123", log_lines = 20 }`).

## Capabilities

A routine reaches only what its header declares:

| Capability | Reaches |
|---|---|
| `github.read` | GitHub queries and REST reads, `github.prs`, and waits on CI, PRs and runs |
| `github.write` | GitHub mutations and REST writes, and jouet's commands that post or merge |
| `git.read` | The work tree's state, diffs, history and refs |
| `fs.read`, `fs.write` | Planning and writing edits; waits on files and logs |
| `net` | Waits on ports and URLs |
| `ledger.read` | The hooks' ledger |
| `mise.read`, `mise.run` | mise's config and tasks, and running them |

A primitive outside them fails with exit `4`, and the error names the line to add. Reading a routine's header is enough to know what running it can do.

### Writes need `--yes`

A routine with a capability that writes (`github.write` or `fs.write`) gets `--yes` and `--dry-run`, and acts only with `--yes`. Without it, every write is previewed instead of made: a GitHub write returns `{ dry_run = true }`, an edit returns its plan, and `jouet pr land` and the thread commands run with `--dry-run`. The run then lists what it would have done and exits `4`, so an agent that forgot `--yes` gets a preview and a clear refusal, never a surprise. `--dry-run` previews the same way and exits as the routine says.

Such a routine must also carry [examples](#testing): without them it refuses to run.

### The sandbox

Routines run in a sandboxed Luau VM: the standard library without `require`, `io` or most of `os`, plus the primitives. The Luau between two primitive calls has a budget of 10 seconds and fifty million interrupts, in 256 MiB; a primitive's own time, such as a long `wait.ci`, does not count.

## Testing

A routine's table can carry `examples`: runs against recorded responses, and what each must come to. `joue test NAME` runs them; `joue test` runs every routine's.

```lua
examples = {
	{
		name = "a conflicted PR is the agent's to act on",
		args = { "7", "--repo", "jdx/demo" },
		fixture = "fixtures/conflict.json",
		exit = 10,
		output = "act  conflict",
		json = { prs = { { action = "conflict" } } },
	},
},
```

| Field | Meaning |
|---|---|
| `args` | The words after the routine's name; `yes = true` adds `--yes` |
| `fixture` | A JSON file beside the routine, or the same table inline |
| `exit` | The exit code (default `0`) |
| `output` | Text the output must contain: a string or a list. An error shows as `jouet: ...` |
| `json` | Fields the `--json` document must have, run separately. A list must have as many entries |
| `planned` | Writes that must have been previewed (`{}` for none) |
| `requests` | Requests that must have been made, as text found in `METHOD PATH BODY` or `jouet COMMAND` |

A fixture holds the answers:

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

`github` is served by a local fake GitHub: each request gets the first unused entry with its method and path whose `match` text is in the body, then the last matching one again, so a poll can repeat. `jouet` answers jouet's commands in order, by the start of their command line. `git`, `mise` and `ledger` answer those primitives by name. Nothing reaches the real GitHub, git or mise, and edits are only ever previewed, so `joue test` is safe to run anywhere. A request with no recorded answer fails the example and says which.

## Choosing a routine, a mise task or a rule

- **A mise task** for fixed commands with no decisions, specific to one project and useful to people too. It is the default: start with one.
- **A routine** once conditionals or parsing creep in: logic over structured state, built on jouet's primitives, with structured output, often reusable across projects.
- **A [rule](/rules)** to steer the agent's own tool calls, typically toward a task or a routine: "use `joue triage` instead of that `gh` loop".

[Self-winding](/winding) can propose any of the three; the agent picks the kind that fits.

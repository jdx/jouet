---
description: Recipes, Luau scripts that sequence jouet's own commands, such as pr next, wait ci and pr land, with every guardrail those commands have.
---

# jouet run

`jouet run` runs a recipe: a short [Luau](https://luau.org) script that sequences jouet's own commands. A recipe turns a routine an agent repeats every session, such as "check the PR, wait for CI, then merge", into one command. Recipes stay thin, because the work is done by the commands they call.

```text
jouet run RECIPE [ARGS...] [--dry-run]
jouet run --list
jouet run --trust | --untrust
```

The flags are listed in the [CLI reference](/cli/run). The words after the recipe's name are its arguments. A word of the form `key=value` sets `args.key`, and `true` or `false` becomes a boolean. Any other word is the next of `args[1]`, `args[2]` and so on.

```console
$ jouet run --list
land        land this branch's PR: pr next, wait for CI if it is still running, then pr land
            args: [PR] [yes=true]: the PR (default: this branch's); yes=true merges
ship-check  before saying done: run the repository's test task, then pr status, and report both
            args: [PR]: the PR (default: this branch's)
sweep       list a PR's unresolved review threads as a compact work list, posting nothing
            args: [PR]: the PR (default: this branch's)

$ jouet run --dry-run land 123 yes=true
land would run:
  1. jouet pr next 123
  2. jouet wait ci 123
  3. jouet pr next 123
  4. jouet pr land 123 --yes
```

`--dry-run` prints each step and runs none of them. Each step gets a stand-in result that succeeded, so the dry run follows the path where everything goes well.

## The built-in recipes

| Recipe | What it does |
|---|---|
| `land [PR] [yes=true]` | Runs `pr next`. If CI is still running, it runs `wait ci` and then `pr next` again. Once the PR is `ready_to_merge`, it runs `pr land`. Without `yes=true` the last step is `pr land --dry-run`, which shows the plan and merges nothing. It stops with the exit code of the step that stopped it, so `20` still means "waiting". |
| `sweep [PR]` | Lists the unresolved review threads as one line each: ID, `path:line`, author, and the first line of the comment. It never posts. |
| `ship-check [PR]` | Runs the repository's test task, then `pr status`, and reports both. It fails when the tests fail or the PR needs attention. Without a PR it reports the tests alone. |

## Where recipes come from

| Source | Directory | Runs |
|---|---|---|
| Built in | embedded in the binary | always |
| Yours | `~/.config/jouet/recipes/*.luau` (`$XDG_CONFIG_HOME/jouet/recipes`) | always |
| A repository's | `.jouet/recipes/*.luau` at the repository root | once trusted with `jouet run --trust` |

A recipe's name is its file name without `.luau`. As with [rules](/rules#where-rules-come-from), a recipe cannot take a built-in one's name. A repository's recipes are trusted by a hash of their files, so adding or changing one needs trusting again.

## Writing a recipe

A recipe starts with a `--- why:` header and returns a table with a `run` function:

```lua
--!strict
--- why: wait for CI on this branch's PR, then say what it needs next
local recipe: JouetRecipe = {
	usage = "[PR]",
	run = function(args)
		local ci = jouet.cmd.wait_ci({ pr = args[1], quiet = true })
		if not ci.ok then
			return { ok = false, exit_code = ci.exit_code, message = ci.error or "CI failed" }
		end
		local next = jouet.cmd.pr_next({ prs = args[1] })
		local pr = next.json.prs[1]
		return `{pr.action}: {pr.reason}`
	end,
}
return recipe
```

`run` returns one of these:

- **`nil`:** the recipe is done.
- **A string:** a message to print.
- **A table:** its `ok` field (true unless it says otherwise), `message` and `exit_code` decide how `jouet run` ends. The whole table is the `result` of the JSON report.

A recipe runs in the same sandbox as a rule, and has the same `jouet` module and [probes](/rules#probes). It also has `jouet.cmd`.

### `jouet.cmd`

Each function in `jouet.cmd` runs one jouet command, with `--json`, and returns its result as `{ ok, exit_code, json, error, command }`. `json` holds the command's JSON document and `error` holds an error document's message:

| Function | Command |
|---|---|
| `pr_status`, `pr_next` | `jouet pr status`, `jouet pr next` |
| `pr_threads` | `jouet pr threads` (listing) |
| `pr_reply`, `pr_resolve` | `jouet pr threads reply`, `jouet pr threads resolve` (post; need `yes = true`) |
| `pr_land` | `jouet pr land` (merges; needs `yes = true`, which it passes on as `--yes`) |
| `wait_ci`, `wait_run`, `wait_pr`, `wait_file`, `wait_log`, `wait_port`, `wait_http`, `wait_pid` | `jouet wait ...` |
| `test` | the repository's test task, found by what the repository has: a mise `test` task, a package.json `test` script, Cargo.toml, a justfile or Makefile `test` target, or go.mod. Its output goes to a log; `json` holds `command`, `exit_code`, `duration_ms`, `log` and `tail`. |

Arguments are a table. Positional arguments go by name (`{ pr = "123" }`) or in order (`{ "123" }`). Flags go by name, with `_` for `-`: `{ log_lines = 20 }` is `--log-lines=20`. A flag that takes no value is `true` or `false`, and one that repeats takes a list (`{ check = { "unit", "lint" } }`). An argument the command does not take is an error.

### Guardrails

A recipe can do nothing its commands could not:

- **Only `jouet.cmd`.** There is no shell and no file access. Commands that would run a shell command, such as `wait cmd`, are not in `jouet.cmd`, and neither is `rule trust`, which is for a person to run.
- **Writes need `yes = true`.** A command that posts or merges (`pr_reply`, `pr_resolve`, `pr_land`) runs only with `yes = true`, or with `dry_run = true`, which the command honours by changing nothing. Without either, the recipe stops with exit code `4` before anything runs. With `yes`, the command still refuses whatever it refuses when run by hand. For example, `pr land` refuses a PR that branch protection blocks.
- **Arguments are checked.** A positional argument cannot start with `-`, and it cannot be a word the command would read as a subcommand.
- **A budget per step.** The Luau code and probes between two commands get 30 seconds and ten million interrupts. A command's own running time, such as a long `wait ci`, does not count. A run starts at most 64 commands.
- **Repository recipes need trust,** like repository rules.

## JSON

With `--json`, `jouet run` prints one document:

```json
{
  "recipe": "land",
  "ok": true,
  "exit_code": 0,
  "message": "merged jdx/demo#7",
  "steps": [
    { "cmd": "pr_next", "command": "jouet pr next", "exit_code": 0 },
    { "cmd": "pr_land", "command": "jouet pr land 7 --yes", "exit_code": 0 }
  ],
  "result": { "ok": true, "message": "merged jdx/demo#7", "land": { "result": "merged" } }
}
```

A dry run adds `"dry_run": true`, and its steps have no `exit_code`. A recipe that failed has `error`.

## Exit codes

| Code | When |
|---|---|
| `0` | The recipe finished and said it was ok |
| `1` | The recipe failed or threw an error, or ran out of budget |
| `3` | There is no recipe with that name |
| `4` | Refused: a write without `yes = true`, or a repository recipe that is not trusted |
| `5`, `10`, `20`, `124` | The `exit_code` the recipe returned: the code of the jouet command that stopped it, such as `20` from `pr next` while it waits on a reviewer |

A recipe can return any `exit_code`, such as a test task's `101`, and the JSON report keeps it. `jouet run` exits with it only when it is one of jouet's own codes, and with `1` otherwise.

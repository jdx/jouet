-- Type definitions for the `jouet` module that jouet rules run against.
-- `jouet rule types` prints this file. Check a rule with luau-lsp:
--
--   jouet rule types > jouet.d.luau
--   luau-lsp analyze --definitions=jouet.d.luau my-rule.luau
--
-- A rule is a `.luau` file that starts with a header and returns a
-- `JouetRule`, with examples of calls it must fire on and near misses it
-- must stay quiet on:
--
--   --- name: my-rule        (optional; must match the file name)
--   --- why: what the rule is for, in one line   (required)
--   --- mode: warn           (warn or deny; a user's rules default to warn)
--   --- scope: user          (optional; must match where the file lives)
--   --- author: <who wrote it: a person, or the session that did>
--   --- created: 2026-10-30
--
-- It runs in a sandboxed Luau VM: the standard library minus `require`, with
-- `jouet` as its only way to see the tool call. A rule of yours or a
-- repository's only adds decisions: it cannot replace a built-in rule, and
-- its denial of a command that only runs jouet becomes a warning.
--
-- A recipe (`jouet run`) is a `.luau` file with the same `--- why:` header
-- that returns a `JouetRecipe`. It runs in the same sandbox, and also has
-- `jouet.cmd` to run jouet's own commands.
--
-- Probes (`jouet.pr`, `jouet.ci`, `jouet.git.state`, `jouet.repo`,
-- `jouet.env`) only read. Each is cached for the rest of the hook call, or of
-- the recipe step, and runs inside the caller's budget. Most return
-- `(value, nil)`, `(nil, reason)` on failure, or `(nil, nil)` when there is
-- nothing to report. The GitHub probes (`jouet.pr`) are off in hooks unless
-- the `rules.network` setting is on; recipes always have them.

-- One simple command, after quote removal.
export type JouetCommand = {
	-- Every word, with leading reserved words (`do`, `then`, ...), variable
	-- assignments and redirections removed.
	words: { string },
	-- `words` without wrappers such as `env FOO=1`, `sudo`, `timeout 60`,
	-- `rtk` and `mise x tool --`, so `argv[1]` is the program that runs.
	argv: { string },
	-- `argv[1]`'s basename: `/usr/bin/git` and `git.exe` are both `git`.
	program: string?,
	-- The bodies of the command's here-documents.
	heredocs: { string },
	-- The operator after the command; nil at the end of the script.
	op: ("seq" | "and" | "or" | "pipe" | "background")?,
	-- Inside `$(...)` or a script passed to `sh -c`.
	nested: boolean,
	-- The exit status this command is known to have had (post-tool-use only):
	-- a failure belongs to the last pipeline, success to the final `&&` chain.
	status: number?,
	-- `argv` joined with spaces, cut to 80 characters, for messages.
	text: string,
}

export type JouetEvent = "pre-tool-use" | "post-tool-use"

-- The tool call a rule is checking.
export type JouetCall = {
	event: JouetEvent,
	-- "claude", "codex", or "test" under `jouet rule test` unless an example
	-- names a harness.
	harness: string,
	-- The harness's tool name, such as "Bash".
	tool: string,
	-- The shell command, for a shell tool.
	command: string?,
	cwd: string?,
	-- The command's exit code (post-tool-use only).
	exit_code: number?,
	-- Every command the script runs: top-level ones first, then those inside
	-- `$(...)` and `sh -c '...'`. Quoted text, here-documents and comments are
	-- never commands.
	commands: { JouetCommand },
}

export type JouetDecision = {
	action: "deny" | "warn" | "allow",
	message: string?,
}

-- A tool call in compact form: a shell command, or a table that also names
-- the tool and harness, which default to "Bash" and "test".
export type JouetExample = string | {
	command: string?,
	tool: string?,
	harness: string?,
	-- For a post-tool-use rule; 0 if omitted.
	exit_code: number?,
	-- For a `fires` example: the action `check` must return, when it matters
	-- whether the rule denies or warns.
	action: ("deny" | "warn")?,
	-- For a `fires` example of a post-tool-use rule: the kind it must
	-- record, "test", "lint" or "build".
	kind: string?,
}

export type JouetRule = {
	-- When the rule runs; "pre-tool-use" if omitted.
	on: JouetEvent?,
	-- A jouet command the rule's advice names, such as { "wait", "ci" }. The
	-- rule is off when the installed jouet does not have it.
	needs: { string }?,
	-- Returns a decision, or nil to say nothing. The first rule to deny wins.
	check: (call: JouetCall) -> JouetDecision?,
	-- Tool calls the rule must fire on (deny, warn or record), and near
	-- misses it must stay quiet on, run by `jouet rule test`. They check what
	-- `check` returns, before the mode turns a deny into a warning. A rule
	-- of yours or a repository's without both kinds only warns, and cannot
	-- be promoted to deny or shared.
	examples: {
		fires: { JouetExample },
		quiet: { JouetExample },
	},
}

-- `jouet pr status --json`'s entry for one PR. Fields that are empty are
-- absent, as in the command's JSON.
export type JouetPrStatus = {
	repo: string,
	number: number,
	title: string,
	url: string,
	state: "open" | "draft" | "merged" | "closed",
	base: string,
	head: string,
	head_sha: string,
	pushed_at: string?,
	mergeable: "mergeable" | "conflicting" | "unknown",
	merge_state: string,
	checks: { pass: number?, fail: number?, pending: number?, skipped: number?, failing: { string }? }?,
	review: string?,
	unresolved_threads: number?,
	auto_merge: boolean?,
	attention: { { code: string, message: string } }?,
}

-- `jouet pr next --json`'s entry for one PR.
export type JouetPrNext = {
	repo: string,
	number: number,
	title: string,
	url: string,
	-- Such as "ready_to_merge", "checks_pending" or "unresolved_threads".
	action: string,
	kind: "done" | "act" | "wait",
	reason: string,
	command: string?,
}

-- The newest test, lint or build run the hooks recorded in the ledger.
export type JouetLastRun = {
	kind: "test" | "lint" | "build",
	-- The rule that recorded it.
	rule: string,
	ok: boolean,
	exit_code: number?,
	-- The command's shape, with values replaced by placeholders.
	shape: string?,
	-- Unix seconds.
	ts: number?,
	duration_ms: number?,
	harness: string?,
}

export type JouetGitState = {
	-- Absent when HEAD is detached.
	branch: string?,
	detached: boolean,
	-- The first 12 digits of HEAD; absent before the first commit.
	head: string?,
	upstream: string?,
	ahead: number?,
	behind: number?,
	-- Anything changed, untracked or conflicted.
	dirty: boolean,
	changed: number,
	untracked: number,
	conflicted: number,
}

export type JouetRepo = {
	-- The work tree's root.
	root: string,
	linked_worktree: boolean,
	-- From the git remotes, chosen as gh chooses; absent without a GitHub
	-- remote.
	owner: string?,
	name: string?,
	-- "owner/name".
	repo: string?,
}

-- What a `jouet.cmd` call returns.
export type JouetResult = {
	ok: boolean,
	exit_code: number,
	-- The command's JSON document; {} when it printed none, or in a dry run.
	json: any,
	-- The error document's message, when the command failed with one.
	error: string?,
	-- The command line it ran.
	command: string,
	-- Under `jouet run --dry-run`: nothing ran, and the result is a stand-in.
	dry_run: boolean?,
}

-- A command's arguments: its positional arguments by name or in order, and
-- its flags by name with `_` for `-` (`log_lines = 20` is `--log-lines=20`).
-- A flag that takes no value is `true` or `false`; one that repeats takes a
-- list. A command that posts or merges also needs `yes = true`, unless it is
-- given `dry_run = true`.
export type JouetArgs = { [any]: any }

-- The commands a recipe can run. Nothing else is reachable: no shell, no
-- files, no `wait cmd` or `rule trust`.
export type JouetCommands = {
	-- jouet pr status [PRS...]: mine, search, repo, settle, fail_on_attention
	pr_status: (args: JouetArgs?) -> JouetResult,
	-- jouet pr next [PRS...]: mine, search, repo, settle
	pr_next: (args: JouetArgs?) -> JouetResult,
	-- jouet pr threads [PR]: listing only; repo, all, resolved, outdated,
	-- no_outdated, author, bots, no_bots, since, path, include_reviews,
	-- context, full, fail_if_unresolved
	pr_threads: (args: JouetArgs?) -> JouetResult,
	-- jouet pr threads reply ID [PR] (posts; needs yes): repo, body,
	-- resolve, ignore_resolved, dry_run
	pr_reply: (args: JouetArgs?) -> JouetResult,
	-- jouet pr threads resolve IDS... (posts; needs yes): repo, reply,
	-- ignore_resolved, dry_run
	pr_resolve: (args: JouetArgs?) -> JouetResult,
	-- jouet pr land [PR] (merges; needs yes, passed on as --yes): repo,
	-- method, update, dry_run, timeout, interval, settle
	pr_land: (args: JouetArgs?) -> JouetResult,
	-- jouet wait ci [PR]: repo, sha, required, check, ignore, all,
	-- log_lines, no_logs, appear_timeout, and every wait's timeout,
	-- interval and quiet
	wait_ci: (args: JouetArgs?) -> JouetResult,
	-- jouet wait run RUN: repo, log_lines, no_logs
	wait_run: (args: JouetArgs?) -> JouetResult,
	-- jouet wait pr [PR]: repo, until
	wait_pr: (args: JouetArgs?) -> JouetResult,
	-- jouet wait file PATH: contains, gone
	wait_file: (args: JouetArgs?) -> JouetResult,
	-- jouet wait log PATH: match, from_start
	wait_log: (args: JouetArgs?) -> JouetResult,
	-- jouet wait port ADDR: closed
	wait_port: (args: JouetArgs?) -> JouetResult,
	-- jouet wait http URL: status, contains
	wait_http: (args: JouetArgs?) -> JouetResult,
	-- jouet wait pid PID
	wait_pid: (args: JouetArgs?) -> JouetResult,
	-- The repository's test task, found by what it has: a mise `test` task,
	-- a package.json `test` script, Cargo.toml, a justfile or Makefile
	-- `test` target, go.mod. `json` holds `command`, `exit_code`,
	-- `duration_ms`, `log` (the full output) and `tail` (its last lines).
	test: () -> JouetResult,
}

-- What `run` returns: nil (done), a message, or a table whose `ok`
-- (default true), `message` and `exit_code` decide how `jouet run` ends.
-- The whole table is the JSON report's `result`.
export type JouetRecipeResult = string | {
	ok: boolean?,
	message: string?,
	exit_code: number?,
	[string]: any,
}

export type JouetRecipe = {
	-- The arguments it takes, in a line, for `jouet run --list`.
	usage: string?,
	-- `args` holds the words after the recipe's name: `key=value` as
	-- fields (`true` and `false` are booleans), the rest as args[1], ...
	run: (args: { [any]: any }) -> JouetRecipeResult?,
}

-- `jouet.git`: called as a function it reads a git command; its `state`
-- probe reads the work tree.
export type JouetGit = typeof(setmetatable(
	{} :: {
		-- The current branch, its upstream and how far apart they are, and
		-- whether the work tree is clean, from `git status`.
		state: () -> (JouetGitState?, string?),
	},
	{} :: {
		-- A git command's subcommand and its arguments, past global options
		-- such as `-C dir`; nil and {} when the command is not git.
		__call: (any, command: JouetCommand) -> (string?, { string }),
	}
))

declare jouet: {
	-- Refuse the tool call; the agent sees the message. In a warn-mode rule
	-- this warns instead.
	deny: (message: string) -> JouetDecision,
	-- Let the call run, and show the agent the message.
	warn: (message: string) -> JouetDecision,
	-- Say explicitly that the call is fine; the same as returning nil.
	allow: () -> JouetDecision,
	-- Record a run of kind "test", "lint" or "build" with the command's
	-- `status`, for the check before the agent stops. Ignored when `status`
	-- is nil; any other kind is an error.
	record: (kind: string, command: JouetCommand) -> (),
	-- Parse a script the way `call.commands` is built; with an exit code,
	-- top-level commands get a `status`.
	parse: (script: string, exit_code: number?) -> { JouetCommand },
	-- Whether the installed jouet has a command, such as ("pr", "threads").
	has_command: (...string) -> boolean,
	-- `jouet.git(command)`: a git command's subcommand and its arguments,
	-- past global options such as `-C dir`; nil and {} when the command is
	-- not git. `jouet.git.state()` is a probe of the work tree.
	git: JouetGit,
	-- A gh command's arguments after `gh`; nil when it is not gh.
	gh: (command: JouetCommand) -> { string }?,
	-- The arguments before a `--` that ends the options.
	options: (args: { string }) -> { string },
	-- Whether a short-option cluster such as "-fu" holds `flag`, stopping at
	-- the first letter in `takes_value`.
	short_flag: (arg: string, flag: string, takes_value: string?) -> boolean,
	contains: (list: { string }, value: string) -> boolean,
	basename: (path: string) -> string,

	-- Probes. The GitHub ones need the network: off in hooks unless
	-- `rules.network` is on.
	pr: {
		-- The PR `pr` names (a number, URL, OWNER/REPO#N or branch), or this
		-- branch's: what `jouet pr status` reports, without waiting for an
		-- unknown mergeable state to settle.
		status: (pr: string?) -> (JouetPrStatus?, string?),
		-- What the PR needs next, as `jouet pr next` says.
		next: (pr: string?) -> (JouetPrNext?, string?),
	},
	ci: {
		-- The newest recorded run of a kind ("test", "lint" or "build"; any
		-- when nil) from the hooks' ledger, passing over runs another
		-- repository recorded. nil when there is no ledger or no such run.
		last_run: (kind: string?) -> (JouetLastRun?, string?),
	},
	-- The work tree's root and its GitHub repository.
	repo: () -> (JouetRepo?, string?),
	-- An environment variable, if it is on the allowlist: AI_AGENT,
	-- CARGO_TARGET_DIR, CI, CLAUDECODE, CODEX_SANDBOX, GITHUB_ACTIONS,
	-- GITHUB_BASE_REF, GITHUB_HEAD_REF, GITHUB_REF_NAME, GITHUB_REPOSITORY,
	-- LANG, MISE_ENV, NODE_ENV, SHELL, TERM, USER, VIRTUAL_ENV, and JOUET_*.
	-- A name with TOKEN, SECRET, PASSWORD or KEY in it never is; any other
	-- name is an error.
	env: (name: string) -> string?,

	-- Recipes only: jouet's commands. In a rule every call is an error.
	cmd: JouetCommands,
}

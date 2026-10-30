-- Type definitions for the `jouet` module that rule scripts run against.
-- `jouet rule types` prints this file. Check a script with luau-lsp:
--
--   jouet rule types > jouet.d.luau
--   luau-lsp analyze --definitions=jouet.d.luau my-rule.luau
--
-- A rule is a `[[rule]]` table in a TOML file (`jouet rule schema`). One
-- with logic a matcher cannot express names a script, `script =
-- "my-rule.luau"`, which returns a `JouetRule` with a `check` function. The
-- rule's name, mode, `on`, `needs` and examples stay in its TOML.
--
-- A script runs in a sandboxed Luau VM: the standard library minus
-- `require`, with `jouet` as its only way to see the tool call. A rule of
-- yours or a repository's only adds decisions: it cannot replace a built-in
-- rule, and its denial of a command that only runs jouet becomes a warning.
--
-- A whole `.luau` rule file, with a `--- why:` header and its `on`, `needs`
-- and `examples` in the table, still loads, but is deprecated: move those
-- into a `[[rule]]` table.
--
-- Probes (`jouet.pr`, `jouet.ci`, `jouet.git.state`, `jouet.git.tree`,
-- `jouet.cargo`, `jouet.repo`, `jouet.env`) only read. Each is cached for
-- the rest of the hook call, and runs inside the caller's budget. Most return
-- `(value, nil)`, `(nil, reason)` on failure, or `(nil, nil)` when there is
-- nothing to report. The GitHub probes (`jouet.pr`) are off in hooks unless
-- the `rules.network` setting is on.

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
	-- The wrappers `argv` looked through, by basename, outermost first:
	-- `rtk git status` ran via { "rtk" }.
	via: { string },
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
	-- The file a file tool reads or writes.
	path: string?,
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
	-- For a file tool: the path it reads or writes.
	path: string?,
	harness: string?,
	-- For a post-tool-use rule; 0 if omitted.
	exit_code: number?,
	-- For a `fires` example: the action `check` must return, when it matters
	-- whether the rule denies or warns.
	action: ("deny" | "warn")?,
	-- For a `fires` example of a post-tool-use rule: the kind it must
	-- record, "test", "lint" or "build".
	kind: string?,
	-- For a `fires` example with a `kind`: the scope it must record, "full",
	-- or "partial" and what the run selected, such as "partial -p jouet".
	scope: string?,
}

export type JouetRule = {
	-- Returns a decision, or nil to say nothing. The first rule to deny wins.
	check: (call: JouetCall) -> JouetDecision?,
	-- The rest is for a deprecated whole-file Luau rule; a script's rule
	-- says these in its TOML.
	-- When the rule runs; "pre-tool-use" if omitted.
	on: JouetEvent?,
	-- A jouet command the rule's advice names, such as { "wait", "ci" }. The
	-- rule is off when the installed jouet does not have it.
	needs: { string }?,
	-- Tool calls the rule must fire on (deny, warn or record), and near
	-- misses it must stay quiet on, run by `jouet rule test`. They check what
	-- `check` returns, before the mode turns a deny into a warning. A rule
	-- of yours or a repository's without both kinds only warns, and cannot
	-- be promoted to deny or shared. They run without probes, so they give
	-- the same answer anywhere: every probe returns nil and a reason.
	examples: {
		fires: { JouetExample },
		quiet: { JouetExample },
	}?,
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
	-- Whether the run covered the whole project. Runs recorded before jouet
	-- kept a scope read as "partial".
	scope: "full" | "partial",
	-- The work tree the run saw, as `jouet.git.tree()` hashes it; absent
	-- when it could not be read, and for older runs.
	tree: string?,
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

-- The Cargo workspace a directory is in.
export type JouetCargoWorkspace = {
	-- The workspace's root directory.
	root: string,
	-- The package whose manifest is nearest the directory: what a cargo
	-- command there builds without `-p` or `--workspace`. Absent at a
	-- virtual manifest.
	package: string?,
	-- Every member's package name, sorted; absent when a `members` pattern
	-- is more than a `*` or `?` in its last part.
	members: { string }?,
	-- Whether the workspace sets `default-members`.
	default_members: boolean,
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

-- `jouet.git`: called as a function it reads a git command; its `state`
-- probe reads the work tree.
export type JouetGit = typeof(setmetatable(
	{} :: {
		-- The current branch, its upstream and how far apart they are, and
		-- whether the work tree is clean, from `git status`.
		state: () -> (JouetGitState?, string?),
		-- A hash of the content of every tracked file in the work tree,
		-- staged or not; untracked and ignored files are left out. A commit
		-- with the same content hashes the same. The `tree` of a recorded run.
		tree: () -> (string?, string?),
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
	-- is nil; any other kind is an error. `scope` is "full" when the run
	-- covered the whole project and "partial" otherwise (nil is partial), and
	-- `selector` says what a partial run selected, such as "-p jouet".
	record: (kind: string, command: JouetCommand, scope: string?, selector: string?) -> (),
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
		-- when nil) and scope ("full" or "partial"; any when nil) from the
		-- hooks' ledger, passing over runs another repository recorded. nil
		-- when there is no ledger or no such run.
		last_run: (kind: string?, scope: string?) -> (JouetLastRun?, string?),
	},
	cargo: {
		-- The Cargo workspace around `dir` (relative to the call's directory;
		-- the call's directory when nil). nil outside a Cargo project.
		workspace: (dir: string?) -> (JouetCargoWorkspace?, string?),
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
}

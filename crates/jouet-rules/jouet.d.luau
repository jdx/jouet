-- Type definitions for the `jouet` module that jouet rules run against.
-- `jouet rule types` prints this file. Check a rule with luau-lsp:
--
--   jouet rule types > jouet.d.luau
--   luau-lsp analyze --definitions=jouet.d.luau my-rule.luau
--
-- A rule is a `.luau` file that returns a `JouetRule`. It runs in a sandboxed
-- Luau VM: the standard library minus `require`, with `jouet` as its only way
-- to see the tool call.

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
	-- "claude", "codex", or "test" under `jouet rule test`.
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

export type JouetExample = string | {
	command: string,
	exit_code: number?,
	-- For `record` examples: the kind the rule must record, "test", "lint"
	-- or "build".
	kind: string?,
}

export type JouetRule = {
	-- One line, shown by `jouet rule list`.
	description: string,
	-- When the rule runs; "pre-tool-use" if omitted.
	on: JouetEvent?,
	-- A jouet command the rule's advice names, such as { "wait", "ci" }. The
	-- rule is off when the installed jouet does not have it.
	needs: { string }?,
	-- Returns a decision, or nil to say nothing. The first rule to deny wins.
	check: (call: JouetCall) -> JouetDecision?,
	-- Commands and what the rule must make of each, run by `jouet rule test`.
	examples: {
		deny: { JouetExample }?,
		warn: { JouetExample }?,
		allow: { JouetExample }?,
		record: { JouetExample }?,
	}?,
}

declare jouet: {
	-- Refuse the tool call; the agent sees the message.
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
	-- A git command's subcommand and its arguments, past global options such
	-- as `-C dir`; nil and {} when the command is not git.
	git: (command: JouetCommand) -> (string?, { string }),
	-- A gh command's arguments after `gh`; nil when it is not gh.
	gh: (command: JouetCommand) -> { string }?,
	-- The arguments before a `--` that ends the options.
	options: (args: { string }) -> { string },
	-- Whether a short-option cluster such as "-fu" holds `flag`, stopping at
	-- the first letter in `takes_value`.
	short_flag: (arg: string, flag: string, takes_value: string?) -> boolean,
	contains: (list: { string }, value: string) -> boolean,
	basename: (path: string) -> string,
}

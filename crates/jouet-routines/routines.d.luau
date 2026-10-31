-- The primitives routines are written against. `joue types` prints this
-- file, for luau-lsp or `luau-analyze --definitions`.
--
-- Each primitive needs a capability in the routine's header; one outside
-- them fails with exit 4. Writes (github.write, fs.write) are previewed
-- unless joue was given --yes.

type Json = any

declare joue: {
	-- --json was given: the returned table is printed, and print() is not.
	json: boolean,
	-- --yes was given: writes are made.
	acting: boolean,
	dry_run: boolean,
	-- The shared exit codes: ok, failed, usage, not_found, precondition,
	-- remote, timeout, and pr next's act and waiting.
	exit: { [string]: number },
	-- Stops the routine with an exit code, a message and a hint.
	fail: (code: number, message: string, hint: string?) -> never,
	-- Marks a table as a JSON list, so an empty one prints as [].
	array: <T>(t: T) -> T,
	-- Marks a table as a JSON object whose keys go in this order.
	object: <T>(t: T, keys: { string }) -> T,
	-- A duration such as 90s or 1h30m, in seconds.
	duration: (text: string) -> (number?, string?),
	-- Unix seconds.
	now: () -> number,
	-- Runs another routine with these words; returns its result and exit
	-- code. It needs no capability the caller lacks.
	call: (name: string, args: { string }?) -> (Json, number),
}

declare github: {
	-- github.read
	repo: () -> { owner: string, name: string, repo: string }?,
	graphql: (query: string, variables: { [string]: Json }?) -> Json,
	get: (path: string, opts: { etag: string? }?) -> { status: number, etag: string?, next: string?, body: Json },
	paginate: (path: string, opts: { max_pages: number? }?) -> { Json },
	prs: (opts: {
		prs: { string }?,
		mine: boolean?,
		search: string?,
		repo: { string }?,
		settle: number?,
	}?) -> { prs: { Json }, not_found: { string }?, incomplete: { Json }?, entire: Json? },
	-- github.write
	mutate: (query: string, variables: { [string]: Json }?) -> Json,
	request: (method: string, path: string, body: Json?) -> { status: number, body: Json },
}

declare git: {
	-- git.read
	state: () -> Json,
	diff: (opts: { base: string?, staged: boolean?, stat: boolean?, paths: { string }? }?) -> string | { { path: string, added: number?, deleted: number? } },
	log: (opts: { range: string?, max: number?, paths: { string }? }?) -> { { sha: string, author: string, date: string, subject: string } },
	refs: (pattern: string?) -> { { name: string, sha: string } },
	rev: (name: string) -> string?,
}

type Ran = { ok: boolean, exit_code: number, json: Json, error: string?, command: string }

declare wait: {
	-- github.read
	ci: (args: { [any]: any }?) -> Ran,
	pr: (args: { [any]: any }?) -> Ran,
	run: (args: { [any]: any }?) -> Ran,
	-- fs.read
	file: (args: { [any]: any }?) -> Ran,
	log: (args: { [any]: any }?) -> Ran,
	-- net
	port: (args: { [any]: any }?) -> Ran,
	http: (args: { [any]: any }?) -> Ran,
	pid: (args: { [any]: any }?) -> Ran,
}

declare jouet: {
	-- github.read
	pr_status: (args: { [any]: any }?) -> Ran,
	pr_next: (args: { [any]: any }?) -> Ran,
	pr_threads: (args: { [any]: any }?) -> Ran,
	-- github.write: run with --yes when joue was, else --dry-run
	pr_reply: (args: { [any]: any }?) -> Ran,
	pr_resolve: (args: { [any]: any }?) -> Ran,
	pr_land: (args: { [any]: any }?) -> Ran,
	-- fs.read: the project's hk checks on files, or the changed files
	diag_check: (args: { [any]: any }?) -> Ran,
	diag_delta: (args: { [any]: any }?) -> Ran,
	-- fs.write: hk's fixers (`hk run fix --safe`); with --yes when joue was, else --dry-run
	diag_fix: (args: { [any]: any }?) -> Ran,
	-- fs.read: the code graph (`jouet graph`), refreshed before each query
	code: {
		symbols: (args: { [any]: any }?) -> Ran,
		callers: (args: { [any]: any }?) -> Ran,
		callees: (args: { [any]: any }?) -> Ran,
		impact: (args: { [any]: any }?) -> Ran,
		explore: (args: { [any]: any }?) -> Ran,
		affected: (args: { [any]: any }?) -> Ran,
	},
	-- fs.read: a dependency's API at the locked version (`jouet docs`), from
	-- its installed sources; { pkg, symbol?, lang?, budget?, private? }
	docs: {
		lookup: (args: { [any]: any }?) -> Ran,
	},
	-- net: packages checked against their registries and OSV (`jouet deps`);
	-- info { packages, refresh? }, audit { ecosystem?, refresh? }
	deps: {
		info: (args: { [any]: any }?) -> Ran,
		audit: (args: { [any]: any }?) -> Ran,
	},
}

declare edit: {
	-- fs.read
	plan: (spec: string, opts: { format: string?, diff: boolean?, expect_hash: { [string]: string }? }?) -> Json,
	-- fs.write
	apply: (spec: string, opts: { format: string?, diff: boolean?, expect_hash: { [string]: string }? }?) -> Json,
}

declare ledger: {
	-- ledger.read
	entries: (opts: { event: string?, since: string?, limit: number?, all_repos: boolean? }?) -> { Json },
	last_run: (kind: string?) -> Json?,
}

type TaskRun = { ok: boolean, exit_code: number, command: { string }, duration_ms: number, log: string, tail: { string } }

declare mise: {
	-- mise.read
	tasks: () -> Json,
	config: () -> Json,
	tools: () -> Json,
	env: () -> Json,
	-- mise.run
	run: (task: string, opts: { args: { string }? }?) -> TaskRun,
	test: () -> TaskRun,
}

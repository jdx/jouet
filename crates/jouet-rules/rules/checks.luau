--!strict
-- The `checks` rule's script (checks.toml): which test, lint and build runs
-- to record, and how much of the suite each covered. It is a script because
-- it reads task names (`mise run test:e2e`, but not `lint-fix`), each
-- tool's subcommands and the packages a cargo run selects, and records
-- rather than denies.

type Kind = "test" | "lint" | "build"

-- A task runner's task name: only its first segment decides.
local function task(name: string?): Kind?
	if not name then
		return nil
	end
	local head = string.match(name, "^[^:%-_]*") or ""
	if head == "test" or head == "t" or head == "ci" or head == "e2e" then
		return "test"
	end
	-- `lint-fix` changes files rather than checking them.
	if (head == "lint" or head == "check" or head == "typecheck") and not string.find(name, "fix", 1, true) then
		return "lint"
	end
	if head == "build" or head == "b" or head == "compile" then
		return "build"
	end
	return nil
end

local CARGO = {
	test = "test",
	t = "test",
	nextest = "test",
	clippy = "lint",
	build = "build",
	b = "build",
	check = "build",
	c = "build",
}

-- A run's scope: whether it covered the whole project, and if not, what it
-- selected. Anything not known to cover everything is partial.
type Scope = { full: boolean, selector: string? }

local FULL: Scope = { full = true }

local function partial(selector: string?): Scope
	return { full = false, selector = if selector and selector ~= "" then selector else nil }
end

-- Keep a directory one verifier word even when it contains whitespace or
-- punctuation. Percent itself is escaped, so different paths stay distinct.
local function scope_path(path: string): string
	return (string.gsub(path, "[^%w%._/%-]", function(c)
		return string.format("%%%02x", string.byte(c) or 0)
	end))
end

local function set(list: { string }): { [string]: boolean }
	local out = {}
	for _, item in list do
		out[item] = true
	end
	return out
end

-- Cargo and nextest options that pick part of the project, with a value.
local CARGO_PICK = set({
	"--test", "--bin", "--example", "--bench", "--exclude", "-E", "--filter-expr", "--filterset",
	"--partition", "--manifest-path", "-C",
})
-- And without one.
local CARGO_PICK_FLAG = set({ "--lib", "--bins", "--tests", "--examples", "--benches", "--doc", "--exact", "--ignored" })
-- Options whose value is the next word, which select nothing.
local CARGO_VALUE = set({
	"--features", "-F", "--target", "--target-dir", "-j", "--jobs", "--color", "--message-format",
	"--profile", "-P", "--config", "-Z", "--retries", "--test-threads", "--failure-output",
	"--success-output", "--status-level", "--final-status-level", "--cargo-profile",
})
-- After `--`: libtest options whose value is the next word.
local LIBTEST_VALUE = set({ "--test-threads", "--format", "--color", "--logfile", "--shuffle-seed", "-Z" })

-- Whether `packages` names every member.
local function covers(packages: { string }, members: { string }): boolean
	local named = set(packages)
	for _, member in members do
		if not named[member] then
			return false
		end
	end
	return true
end

-- The scope of a cargo command, from its arguments after the subcommand.
-- `dir` is where it runs, relative to the call's directory; false when a
-- `cd` went somewhere the rule cannot follow. `libtest` says the words
-- after `--` go to the test harness, where they can filter tests; for
-- clippy and rustfmt they are lint options.
local function cargo_scope(args: { string }, dir: string | false | nil, libtest: boolean): Scope
	local packages: { string } = {}
	local picks: { string } = {}
	local whole = false
	local passthrough = false
	local i = 1
	while i <= #args do
		local arg = args[i]
		i += 1
		local name, value = string.match(arg, "^(%-%-[%w%-]+)=(.*)$")
		if passthrough then
			if not libtest then
				continue
			elseif arg == "--skip" then
				table.insert(picks, `--skip {args[i] or ""}`)
				i += 1
			elseif arg == "--exact" or arg == "--ignored" then
				table.insert(picks, arg)
			elseif LIBTEST_VALUE[arg] then
				i += 1
			elseif string.sub(arg, 1, 1) ~= "-" then
				table.insert(picks, arg)
			end
		elseif arg == "--" then
			passthrough = true
		elseif arg == "--workspace" or arg == "--all" then
			whole = true
		elseif arg == "-p" or arg == "--package" then
			table.insert(packages, args[i] or "")
			i += 1
		elseif name == "--package" then
			table.insert(packages, value or "")
		elseif string.sub(arg, 1, 2) == "-p" and #arg > 2 and string.sub(arg, 3, 3) ~= "-" then
			table.insert(packages, string.sub(arg, 3))
		elseif arg == "--run-ignored" then
			-- `all` runs more, not less.
			if args[i] ~= "all" then
				table.insert(picks, `{arg} {args[i] or ""}`)
			end
			i += 1
		elseif CARGO_PICK[arg] then
			table.insert(picks, `{arg} {args[i] or ""}`)
			i += 1
		elseif name and CARGO_PICK[name] then
			table.insert(picks, arg)
		elseif CARGO_PICK_FLAG[arg] then
			table.insert(picks, arg)
		elseif CARGO_VALUE[arg] then
			i += 1
		elseif string.sub(arg, 1, 1) ~= "-" then
			-- A test name filter, or a value the rule does not know an
			-- option takes: either way, not known to be everything.
			table.insert(picks, arg)
		end
	end
	local named = if #packages > 0 then "-p " .. table.concat(packages, " -p ") else nil
	if #picks > 0 then
		return partial(table.concat(if named then { named, table.concat(picks, " ") } else picks, " "))
	end
	if dir == false then
		return partial(named)
	end
	if whole then
		return FULL
	end
	local ws = jouet.cargo.workspace(dir or nil)
	if named then
		return if ws and ws.members and covers(packages, ws.members) then FULL else partial(named)
	end
	if not ws then
		return partial(nil)
	end
	if not ws.package then
		-- A virtual manifest builds its default members.
		return if ws.default_members then partial("default-members") else FULL
	end
	if ws.members and #ws.members == 1 and ws.members[1] == ws.package then
		return FULL
	end
	return partial(`-p {ws.package}`)
end

-- A task runner's task: the whole of it is the task alone, with nothing
-- passed to it, and a name with no `:`, `-` or `_` part (`test`, not
-- `test:unit`).
local function task_scope(args: { string }, at: number): Scope
	local name = args[at] or ""
	if #args == at and string.match(name, "^%w+$") then
		return FULL
	end
	return partial(table.concat(args, " ", at))
end

-- `go test`, `go vet` and `go build`: everything is `./...` and no `-run`.
local GO_PICK = set({ "-run", "-skip", "-short", "-bench" })
local GO_VALUE = set({
	"-count", "-timeout", "-p", "-parallel", "-tags", "-coverprofile", "-cpu", "-benchtime", "-o",
	"-exec", "-covermode", "-coverpkg", "-vet", "-ldflags", "-gcflags",
})
local function go_scope(args: { string }): Scope
	local packages: { string } = {}
	local picks: { string } = {}
	local i = 2
	while i <= #args do
		local arg = args[i]
		i += 1
		local name = string.match(arg, "^(%-%-?[%w]+)=") or arg
		name = string.gsub(name, "^%-%-", "-")
		if GO_PICK[name] then
			if name == arg and name ~= "-short" then
				table.insert(picks, `{arg} {args[i] or ""}`)
				i += 1
			else
				table.insert(picks, arg)
			end
		elseif GO_VALUE[name] then
			if name == arg then
				i += 1
			end
		elseif string.sub(arg, 1, 1) ~= "-" then
			table.insert(packages, arg)
		end
	end
	if #picks == 0 and #packages == 1 and packages[1] == "./..." then
		return FULL
	end
	local selected = table.concat(packages, " ")
	if #picks > 0 then
		selected = selected .. (if selected == "" then "" else " ") .. table.concat(picks, " ")
	end
	return partial(selected)
end

-- A test runner run with no paths and no name filter runs everything it is
-- configured to. `pick` options select tests; `value` options take the next
-- word.
local RUNNER_PICK = set({
	"-k", "-m", "--lf", "--last-failed", "--deselect", "--sw", "--stepwise", "-t", "--testNamePattern",
	"--onlyChanged", "-o", "--changed", "--related", "--findRelatedTests", "--shard", "--project",
	"--tag", "-e", "--example", "--filter", "--tests", "--grep", "-g", "--exclude",
})
local RUNNER_VALUE = set({
	"-n", "-p", "-c", "--config", "--maxfail", "--rootdir", "--tb", "-W", "--durations", "--basetemp",
	"--junitxml", "--reporter", "--maxWorkers", "-w", "--pool", "--format", "-f", "--require", "-r",
})
local VITEST_PICK = set({ "-c", "--config", "-r", "--root", "--typecheck.only" })
local function runner_scope(args: { string }, from: number, vitest: boolean?): Scope
	local picks: { string } = {}
	local i = from
	while i <= #args do
		local arg = args[i]
		i += 1
		local name = string.match(arg, "^(%-%-?[%w%.%-]+)=") or arg
		if RUNNER_PICK[name] or (vitest and VITEST_PICK[name]) then
			local boolean = name == "--lf" or name == "--last-failed" or name == "--sw" or name == "--stepwise" or name == "--onlyChanged" or name == "--changed" or name == "--typecheck.only"
			table.insert(picks, if name == arg and not boolean then `{arg} {args[i] or ""}` else arg)
			if name == arg and not boolean then
				i += 1
			end
		elseif RUNNER_VALUE[name] then
			if name == arg then
				i += 1
			end
		elseif string.sub(arg, 1, 1) ~= "-" then
			table.insert(picks, arg)
		elseif vitest then
			-- An unknown Vitest option might narrow the run. Never turn it into
			-- an unfiltered pass merely because this rule does not know it.
			table.insert(picks, arg)
		end
	end
	return if #picks == 0 then FULL else partial(table.concat(picks, " "))
end

-- A pnpm directory is a package selection, even when its test script or
-- Vitest runs its entire own suite. Keep it separate from other projects and
-- from other runners so its pass cannot clear a different project's failure.
local function pnpm_scope(args: { string }, scoped_test: boolean?): (Kind?, Scope)
	local unknown = partial(nil)
	local dir = "."
	local at = 1
	if args[1] == "--dir" or args[1] == "-C" then
		dir = args[2] or ""
		at = 3
	elseif args[1] and string.match(args[1], "^%-%-dir=") then
		dir = string.sub(args[1], 7)
		at = 2
	end
	-- Dynamic or ambiguous paths cannot safely share a recorded selection.
	if not string.match(dir, "^[%w%._/%-]+$") then
		return nil, unknown
	end
	local prefix = `@jouet:pnpm:dir={dir}:`
	if args[at] == "exec" and args[at + 1] == "vitest" then
		-- Selection strings are compared as words by the verifier. Do not
		-- conflate one quoted or dynamic argument with several other words.
		for i = at + 2, #args do
			if args[i] == "" or string.find(args[i], "[^%w%._/%-:=@]") then
				return nil, unknown
			end
		end
		-- A Vitest mode is not a file filter: a passing ordinary run does not
		-- cover a failed benchmark, type check, or watch-mode invocation.
		local subcommand = args[at + 2]
		local modes = set({ "bench", "typecheck", "watch", "dev", "related", "list", "init" })
		local mode = if subcommand and modes[subcommand] then `:{subcommand}` else ""
		local from = if subcommand == "run" or mode ~= "" then at + 3 else at + 2
		local scope = runner_scope(args, from, true)
		return "test", partial(prefix .. "vitest" .. mode .. (if scope.selector then " " .. scope.selector else ""))
	elseif (at > 1 or scoped_test) and (args[at] == "test" or args[at] == "t" or ((args[at] == "run" or args[at] == "run-script") and (args[at + 1] == "test" or args[at + 1] == "t"))) then
		local test_at = if args[at] == "run" or args[at] == "run-script" then at + 1 else at
		for i = test_at + 1, #args do
			if args[i] == "" or string.find(args[i], "[^%w%._/%-:=@]") then
				return nil, unknown
			end
		end
		local scope = task_scope(args, test_at)
		return "test", partial(prefix .. "test" .. (if scope.selector then " " .. scope.selector else ""))
	end
	return nil, unknown
end

-- `python -m unittest`: with no test named, or `discover` with its default
-- pattern, it runs every test it finds. `-k` and a `discover` pattern
-- select some; options that take a value are skipped.
local UNITTEST_VALUE = set({ "--durations", "-s", "--start-directory", "-t", "--top-level-directory" })
local function unittest_scope(args: { string }, from: number): Scope
	local picks: { string } = {}
	local positional: { string } = {}
	local discover = args[from] == "discover"
	local i = if discover then from + 1 else from
	while i <= #args do
		local arg = args[i]
		i += 1
		local name = string.match(arg, "^(%-%-?[%w%-]+)=") or arg
		if name == "-k" or name == "-p" or name == "--pattern" then
			table.insert(picks, if name == arg then `{arg} {args[i] or ""}` else arg)
			if name == arg then
				i += 1
			end
		elseif UNITTEST_VALUE[name] then
			if name == arg then
				i += 1
			end
		elseif string.sub(arg, 1, 1) ~= "-" then
			table.insert(positional, arg)
		end
	end
	-- `discover START PATTERN TOP`: only a pattern selects.
	if discover then
		if positional[2] then
			table.insert(picks, positional[2])
		end
	else
		for _, name in positional do
			table.insert(picks, name)
		end
	end
	return if #picks == 0 then FULL else partial(table.concat(picks, " "))
end

-- hk options that take a value, that narrow what a check covers, and that
-- print a plan or help instead of checking.
local HK_VALUE = set({
	"--step", "-S", "--skip-step", "--glob", "-g", "--exclude", "-e", "--from-ref", "--to-ref",
	"--files0-from", "--stash", "--profile", "-p", "--jobs", "-j", "--cd", "--format", "--sarif",
	"--junit-xml",
})
local HK_NARROW = set({
	"--step", "-S", "--skip-step", "--glob", "-g", "--exclude", "-e", "--from-ref", "--to-ref",
	"--pr", "--staged", "--unstaged",
})
local HK_INSPECT = set({ "--help", "-h", "--plan", "-P", "--why", "-W" })

-- The scope of `hk check` or `hk run check`, from its arguments after the
-- hook. Without files or options that narrow it, hk checks the changed
-- files, or with `--all` everything. A file list read from a file is
-- recorded as `files0-from FILE`, which the hook reads to tell whether it
-- named every changed file; one read from stdin is a selection the rule
-- cannot name, so it clears only a run of the same command.
local function hk_scope(args: { string }, from: number): (Kind?, Scope)
	local files: { string } = {}
	local narrow: { string } = {}
	local all = false
	local stdin = false
	local list: string? = nil
	local i = from
	while i <= #args do
		local arg = args[i]
		i += 1
		local name = string.match(arg, "^(%-%-[%w%-]+)=") or arg
		local takes = name == arg and HK_VALUE[name]
		if HK_INSPECT[name] then
			return nil, partial(nil)
		elseif name == "--all" or name == "-a" then
			all = true
		elseif name == "--files0-from" then
			local value = if takes then args[i] else string.sub(arg, #name + 2)
			if value == nil or value == "" or value == "-" or string.sub(value, 1, 5) == "/dev/" then
				stdin = true
			else
				list = value
			end
		elseif HK_NARROW[name] then
			table.insert(narrow, if takes then `{arg} {args[i] or ""}` else arg)
		elseif string.sub(arg, 1, 1) ~= "-" then
			table.insert(files, arg)
		end
		if takes then
			i += 1
		end
	end
	if stdin then
		return "lint", partial(nil)
	end
	local selected: { string } = {}
	if #files > 0 then
		table.insert(selected, "files " .. table.concat(files, " "))
	end
	if list then
		table.insert(selected, "files0-from " .. list)
	end
	for _, n in narrow do
		table.insert(selected, n)
	end
	if #selected > 0 then
		return "lint", partial(table.concat(selected, " "))
	end
	return "lint", if all then FULL else partial("changed files")
end

-- jouet's subcommand, past its output options (`jouet --json diag ...`).
local function jouet_subcommand(args: { string }): string?
	local i = 1
	while i <= #args do
		local arg = args[i]
		if arg == "--display" then
			i += 2
		elseif arg == "--json" or string.sub(arg, 1, 10) == "--display=" then
			i += 1
		else
			return arg
		end
	end
	return nil
end

-- `jouet diag check` runs `hk run check` on the files given, or on the
-- changed files. `diag delta` exits 0 with findings that were already there,
-- so it says nothing about whether the check passed, and is not recorded;
-- nor is help.
local function diag_scope(args: { string }): (Kind?, Scope)
	local files: { string } = {}
	local sub: string? = nil
	local seen_diag = false
	local i = 1
	while i <= #args do
		local arg = args[i]
		i += 1
		if arg == "--display" or arg == "--max" then
			i += 1
		elseif arg == "--help" or arg == "-h" then
			return nil, partial(nil)
		elseif string.sub(arg, 1, 1) == "-" then
			-- `--json`, `--changed`, `--max=N`, `--display=MODE`
		elseif not seen_diag then
			seen_diag = arg == "diag"
		elseif not sub then
			sub = arg
		else
			table.insert(files, arg)
		end
	end
	if sub ~= "check" then
		return nil, partial(nil)
	end
	return "lint", partial(if #files > 0 then "files " .. table.concat(files, " ") else "changed files")
end

-- `jouet test`: the full suite, unless `--affected` or words after `--`
-- narrow it; the selector names them. A dry run and help ran no tests.
-- With `--cmd`, the words after `--` are the command it ran, returned
-- third for the caller to classify.
local function jouet_test(args: { string }): (Kind?, Scope, { string }?)
	local unknown = partial(nil)
	local picks: { string } = {}
	local selects: { string } = {}
	local affected = false
	local cmd = false
	local sub = false
	local i = 1
	while i <= #args do
		local arg = args[i]
		i += 1
		if arg == "--" then
			if sub and cmd then
				return nil, unknown, table.move(args, i, #args, 1, {})
			end
			if sub and i <= #args then
				table.insert(picks, "-- " .. table.concat(args, " ", i))
			end
			break
		elseif arg == "--cmd" then
			cmd = true
		elseif arg == "--budget" then
			i += 1
		elseif arg == "--display" then
			i += 1
		elseif arg == "--json" or string.sub(arg, 1, 10) == "--display=" then
			-- Output options, before or after the subcommand.
		elseif not sub then
			if arg ~= "test" then
				return nil, unknown
			end
			sub = true
		elseif arg == "--dry-run" or arg == "--help" or arg == "-h" then
			return nil, unknown
		elseif arg == "--affected" then
			affected = true
		elseif arg == "--since" or arg == "--adapter" then
			table.insert(selects, `{arg} {args[i] or ""}`)
			i += 1
		elseif string.match(arg, "^%-%-since=") or string.match(arg, "^%-%-adapter=") then
			table.insert(selects, arg)
		end
	end
	if not sub then
		return nil, unknown
	end
	-- `--since` and `--adapter` narrow only what `--affected` picks; with
	-- `--baseline`, `--since` names the base to compare with.
	if affected then
		for k, select in selects do
			table.insert(picks, k, select)
		end
		table.insert(picks, 1, "--affected")
	end
	return "test", if #picks == 0 then FULL else partial(table.concat(picks, " "))
end

-- Options that list or explain rather than run: `pytest --collect-only`,
-- `cargo test -- --list`, `--help`. A run with one runs no tests.
local INSPECT = set({
	"--help", "-h", "--version", "-V", "--list", "--collect-only", "--co", "--listTests",
	"--list-tests",
})

-- Only wrappers that explicitly execute the Playwright binary. Package scripts
-- named `playwright` can do anything, so `yarn run playwright` is not one.
local function playwright_args(program: string, args: { string }): (number?, string?)
	local at: number? = nil
	local workspace: string? = nil
	if program == "playwright" then
		at = 1
	elseif program == "npx" or program == "bunx" then
		local i = if args[1] == "--no-install" then 2 else 1
		if args[i] == "playwright" then at = i + 1 end
	elseif (program == "pnpm" or program == "yarn") and args[1] == "exec" and args[2] == "playwright" then
		at = 3
	elseif program == "npm" and args[1] == "exec" and args[2] == "--" and args[3] == "playwright" then
		at = 4
	elseif program == "yarn" and args[1] == "workspace" and args[2] and args[3] == "exec" and args[4] == "playwright" then
		at = 5
		workspace = args[2]
	end
	if at and args[at] == "test" then return at, workspace end
	return nil, nil
end

local function playwright_scope(args: { string }, at: number, workspace: string?): (Kind?, Scope)
	local unknown = partial(nil)
	local custom_config = false
	for i = at + 1, #args do
		local arg = args[i]
		local name = string.match(arg, "^(%-%-?[%w%-]+)=") or arg
		if name == "--config" or name == "-c" then
			custom_config = true
		end
		-- UI modes can exit without running; this must take precedence over
		-- any earlier selection flag, regardless of argument order.
		if name == "--ui" or name == "--ui-host" or name == "--ui-port" or name == "--debug" or name == "--pass-with-no-tests" then
			return nil, unknown
		end
	end
	-- The namespace distinguishes these selections from test names for other
	-- runners. A custom config may name a different suite, so it gets its own
	-- conservative, exact selection rather than being covered by the default.
	local suite = "@jouet:playwright" .. (if workspace then ":workspace=" .. workspace else "")
	if custom_config then suite ..= ":config" end
	local words: { string } = {}
	for i = at + 1, #args do
		local word = args[i]
		-- Preserve word boundaries for the verifier's coverage comparison.
		if word == "" or string.find(word, "[^%w%._/%-:=@]") then
			word = "'" .. string.gsub(word, "'", "'\\''") .. "'"
		end
		table.insert(words, word)
	end
	local selection = table.concat(words, " ")
	-- Even an unfiltered Playwright run need not cover a project's unit tests.
	return "test", partial(suite .. (if selection ~= "" then " " .. selection else ""))
end

-- A script's own commands, without those in `$(...)`, which run in a
-- subshell and are part of a word.
local function top_level(script: { JouetCommand }): { JouetCommand }
	local top: { JouetCommand } = {}
	for _, command in script do
		if not command.nested then
			table.insert(top, command)
		end
	end
	return top
end

local function classify(command: JouetCommand, dir: string | false | nil, cwd: string?, harness: string?): (Kind?, Scope)
	local program: string = command.program or ""
	local args: { string } = table.move(command.argv, 2, #command.argv, 1, {})
	for _, arg in args do
		if INSPECT[arg] then
			return nil, partial(nil)
		end
	end
	-- `cargo +nightly test`: the toolchain comes first.
	if args[1] and string.sub(args[1], 1, 1) == "+" then
		table.remove(args, 1)
	end
	local first: string? = args[1]
	local unknown = partial(nil)
	local playwright_at, workspace = playwright_args(program, args)
	if playwright_at then
		return playwright_scope(args, playwright_at, workspace)
	end
	if program == "pnpm" then
		local is_test = first == "test" or first == "t" or ((first == "run" or first == "run-script") and (args[2] == "test" or args[2] == "t"))
		local scoped_test = dir ~= nil
		if is_test and not scoped_test then
			local repo = if cwd then jouet.repo() else nil
			scoped_test = if cwd then not repo or cwd ~= repo.root else harness ~= "test"
		end
		if (scoped_test and is_test) or first == "exec" or first == "--dir" or first == "-C" or (first and string.match(first, "^%-%-dir=")) then
			return pnpm_scope(args, scoped_test)
		end
	end
	if program == "cargo" or program == "mbx" or program == "cross" then
		if first == "fmt" then
			if not jouet.contains(args, "--check") then
				return nil, unknown
			end
			return "lint", cargo_scope(table.move(args, 2, #args, 1, {}), dir, false)
		end
		local kind: Kind? = if first then CARGO[first] :: Kind? else nil
		if not kind then
			return nil, unknown
		end
		-- `cargo nextest run`: the options follow `run`.
		local from = if first == "nextest" and (args[2] == "run" or args[2] == "r") then 3 else 2
		return kind, cargo_scope(table.move(args, from, #args, 1, {}), dir, kind == "test")
	elseif program == "mise" and (first == "run" or first == "r") then
		return task(args[2]), task_scope(args, 2)
	elseif program == "npm" or program == "pnpm" or program == "yarn" or program == "bun" or program == "aube" then
		if first == "run" or first == "run-script" then
			return task(args[2]), task_scope(args, 2)
		end
		return if first == "test" or first == "t" then "test" else nil, task_scope(args, 1)
	elseif program == "make" or program == "just" or program == "task" then
		return task(first), task_scope(args, 1)
	elseif program == "go" then
		local kind: Kind? = if first == "test" then "test" elseif first == "vet" then "lint" elseif first == "build" then "build" else nil
		return kind, go_scope(args)
	elseif program == "hk" then
		-- `hk run fix` and `hk run pre-commit` change files rather than
		-- check them; `hk run check` checks, like `hk check`.
		local hook = if first == "run" then args[2] else first
		if hook ~= "check" then
			return nil, unknown
		end
		return hk_scope(args, if first == "run" then 3 else 2)
	elseif program == "jouet" and (command.status == 2 or command.status == 4) then
		-- A usage error (2) or a refusal (4), such as `diag check` with no
		-- hk config or `test` with no runner found, checked nothing: it
		-- neither fails a check nor clears an earlier failure.
		return nil, unknown
	elseif program == "jouet" and jouet_subcommand(args) == "diag" then
		return diag_scope(args)
	elseif program == "pytest" or program == "vitest" or program == "jest" or program == "rspec" then
		-- `vitest run` is vitest without watching.
		return "test", runner_scope(args, if program == "vitest" and first == "run" then 2 else 1, program == "vitest")
	elseif program == "jouet" then
		local kind: Kind?, scope: Scope, given: { string }? = jouet_test(args)
		if given and #given > 0 then
			-- `jouet test --cmd -- CMD` ran CMD: a test or a build, with its
			-- scope. A command the rule does not know is recorded as nothing,
			-- as it is when run directly: called a partial test, a passing run
			-- of the whole suite would hold up finishing.
			-- Through its wrappers, as a command's own argv is: `jouet test
			-- --cmd -- mise x -- cargo test` ran cargo.
			local argv, via = jouet.unwrap(given)
			-- One command in a `sh -c` script is that command, its variable
			-- assignments set: `jouet test --cmd -- sh -c 'PATH=… go test'`
			-- ran go test, and its pass clears that test's failure.
			local shell = if argv[1] then jouet.basename(argv[1]) else nil
			if (shell == "sh" or shell == "bash" or shell == "zsh") and argv[2] == "-c" and argv[3] and #argv == 3 then
				local top = top_level(jouet.parse(argv[3], command.status))
				if #top == 1 then
					-- Its own status is unknown when the script says nothing
					-- of it: negated with `!`, or piped into another command
					-- without pipefail.
					if command.status ~= nil and top[1].status == nil then
						return nil, unknown
					end
					return classify(top[1], dir, cwd, harness)
				end
			end
			local inner: JouetCommand = {
				words = given,
				argv = argv,
				program = if argv[1] then jouet.basename(argv[1]) else nil,
				heredocs = {},
				env = {},
				matched = false,
				via = via,
				nested = command.nested,
				status = command.status,
				text = command.text,
				line = command.line,
			}
			local inner_kind: Kind?, inner_scope: Scope = classify(inner, dir, cwd, harness)
			return inner_kind, inner_scope
		end
		return kind, scope
	elseif program == "bats" then
		return "test", unknown
	elseif (program == "python" or program == "python3") and first == "-m" and args[2] == "pytest" then
		return "test", runner_scope(args, 3)
	elseif (program == "python" or program == "python3") and first == "-m" and args[2] == "unittest" then
		return "test", unittest_scope(args, 3)
	elseif (program == "python" or program == "python3") and first == "-m" and args[2] == "tornado.test.runtests" then
		-- Tornado has its own options and test selection. Keep its scope
		-- unknown rather than treating a selected run as the full suite.
		return "test", unknown
	elseif program == "ruff" then
		local kind: Kind? = if first == "check" then "lint" else nil
		return kind, if #args == 1 or (#args == 2 and args[2] == ".") then FULL else unknown
	elseif program == "eslint" or program == "mypy" or program == "shellcheck" then
		return "lint", unknown
	elseif program == "actionlint" then
		-- With no files, actionlint checks every workflow.
		return "lint", if #args == 0 then FULL else unknown
	elseif program == "tsc" then
		return "build", runner_scope(args, 1)
	elseif program == "dotnet" or program == "swift" or program == "gradle" or program == "gradlew" or program == "mvn" then
		local kind: Kind? = if first == "test" then "test" elseif first == "build" or first == "compile" then "build" else nil
		return kind, runner_scope(args, 2)
	end
	return nil, unknown
end

-- The commands of the `sh -c` script `jouet test --cmd --` ran, when it has
-- more than one, passed, stays in the work tree it started in, and runs
-- only checks, each with its own status as if it ran directly: `sh -c
-- 'cargo build && cargo test'` that passed ran both, and passed both. A
-- failed script says nothing of which command failed, and a command that
-- is not a check may change the files after the checks ran. One command is
-- classified as itself (see `classify`).
local function script_commands(command: JouetCommand, dir: string?, cwd: string?, harness: string?): { JouetCommand }?
	if command.program ~= "jouet" or command.status ~= 0 then
		return nil
	end
	local args: { string } = table.move(command.argv, 2, #command.argv, 1, {})
	local _, _, given = jouet_test(args)
	if not given or #given == 0 then
		return nil
	end
	local argv = jouet.unwrap(given)
	local shell = if argv[1] then jouet.basename(argv[1]) else nil
	if not ((shell == "sh" or shell == "bash" or shell == "zsh") and argv[2] == "-c" and argv[3] and #argv == 3) then
		return nil
	end
	-- A wrapper that changes directory (`env -C`, `mise x -C`) ran the
	-- script in a work tree the rule cannot name.
	for i = 1, #given - #argv do
		local word = given[i]
		if string.sub(word, 1, 2) == "-C" or word == "--cd" or word == "--chdir" or string.sub(word, 1, 8) == "--chdir=" or string.sub(word, 1, 5) == "--cd=" then
			return nil
		end
	end
	-- A command in `$(...)` runs in a subshell: its `cd` moves nothing, and
	-- it is not one of the script's checks.
	local script = top_level(jouet.parse(argv[3], command.status))
	if #script < 2 then
		return nil
	end
	for _, inner in script do
		if inner.program == "cd" or inner.program == "pushd" then
			return nil
		end
		-- Each check must be known to have passed: not negated with `!`,
		-- nor piped into another command without pipefail.
		if not classify(inner, dir, cwd, harness) or inner.status ~= 0 then
			return nil
		end
	end
	return script
end

local rule: JouetRule = {
	check = function(call: JouetCall): JouetDecision?
		local dirs = jouet.dirs(call.commands)
		for i, command in call.commands do
			if command.program ~= "cd" or command.nested then
				local dir = dirs[i]
				-- After a `cd` it cannot follow, the run is in a work tree
				-- the rule cannot name, so it is not recorded against this one.
				if dir == false then
					continue
				end
				local project_dir = if type(dir) == "string" then dir else nil
				for _, ran in ipairs(script_commands(command, project_dir, call.cwd, call.harness) or { command }) do
					local kind, scope = classify(ran, project_dir, call.cwd, call.harness)
					if kind then
						if scope.selector and string.sub(scope.selector, 1, 12) == "@jouet:pnpm:" then
							-- Identical --dir arguments can refer to different projects
							-- when calls start in different directories.
							local cwd = call.cwd or (if call.harness == "test" then "." else "unknown")
							local cd = project_dir or "."
							if cwd == "" or cd == "" or string.sub(cd, 1, 1) == "~" or string.find(cd, "[%$`*?%[%]]") then
								continue
							end
							if call.cwd or project_dir or call.harness ~= "test" then
								scope = partial(`@jouet:cwd={scope_path(cwd)}:cd={scope_path(cd)} {scope.selector}`)
							end
						end
						-- Ignored when the command's own exit status is unknown.
						jouet.record(kind, ran, if scope.full then "full" else "partial", scope.selector, project_dir)
					end
				end
			end
		end
		return nil
	end,
}

return rule

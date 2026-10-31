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
	"--tag", "-e", "--example", "--filter", "--tests", "--grep", "-g",
})
local RUNNER_VALUE = set({
	"-n", "-p", "-c", "--config", "--maxfail", "--rootdir", "--tb", "-W", "--durations", "--basetemp",
	"--junitxml", "--reporter", "--maxWorkers", "-w", "--pool", "--format", "-f", "--require", "-r",
})
local function runner_scope(args: { string }, from: number): Scope
	local picks: { string } = {}
	local i = from
	while i <= #args do
		local arg = args[i]
		i += 1
		local name = string.match(arg, "^(%-%-?[%w%-]+)=") or arg
		if RUNNER_PICK[name] then
			table.insert(picks, if name == arg then `{arg} {args[i] or ""}` else arg)
			if name == arg and name ~= "--lf" and name ~= "--last-failed" and name ~= "--sw" and name ~= "--stepwise" and name ~= "--onlyChanged" and name ~= "--changed" then
				i += 1
			end
		elseif RUNNER_VALUE[name] then
			if name == arg then
				i += 1
			end
		elseif string.sub(arg, 1, 1) ~= "-" then
			table.insert(picks, arg)
		end
	end
	return if #picks == 0 then FULL else partial(table.concat(picks, " "))
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
-- files, or with `--all` everything. A file list read from stdin is a
-- selection the rule cannot name, so it clears only a run of the same
-- command.
local function hk_scope(args: { string }, from: number): (Kind?, Scope)
	local files: { string } = {}
	local narrow: { string } = {}
	local all = false
	local stdin = false
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
			stdin = true
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
			table.insert(picks, `{arg} {args[i] or ""}`)
			i += 1
		elseif string.match(arg, "^%-%-since=") or string.match(arg, "^%-%-adapter=") then
			table.insert(picks, arg)
		end
	end
	if not sub then
		return nil, unknown
	end
	if affected then
		table.insert(picks, 1, "--affected")
	end
	return "test", if #picks == 0 then FULL else partial(table.concat(picks, " "))
end

local function classify(command: JouetCommand, dir: string | false | nil): (Kind?, Scope)
	local program: string = command.program or ""
	local args: { string } = table.move(command.argv, 2, #command.argv, 1, {})
	-- `cargo +nightly test`: the toolchain comes first.
	if args[1] and string.sub(args[1], 1, 1) == "+" then
		table.remove(args, 1)
	end
	local first: string? = args[1]
	local unknown = partial(nil)
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
		return "test", runner_scope(args, if program == "vitest" and first == "run" then 2 else 1)
	elseif program == "jouet" then
		local kind, scope, given = jouet_test(args)
		if given and #given > 0 then
			-- `jouet test --cmd -- CMD` ran CMD: a test or a build, with its
			-- scope. A command the rule does not know is recorded as nothing,
			-- as it is when run directly: called a partial test, a passing run
			-- of the whole suite would hold up finishing.
			local inner: JouetCommand = {
				words = given,
				argv = given,
				program = jouet.basename(given[1]),
				heredocs = {},
				via = {},
				nested = command.nested,
				status = command.status,
				text = command.text,
			}
			local inner_kind, inner_scope = classify(inner, dir)
			return inner_kind, inner_scope
		end
		return kind, scope
	elseif program == "bats" then
		return "test", unknown
	elseif (program == "python" or program == "python3") and first == "-m" and args[2] == "pytest" then
		return "test", runner_scope(args, 3)
	elseif (program == "python" or program == "python3") and first == "-m" and args[2] == "unittest" then
		return "test", unittest_scope(args, 3)
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

-- Where `cd DIR` leaves the commands after it: a path relative to the
-- call's directory, or false when it cannot be followed (`cd`, `cd -`,
-- `cd ~/x`, `cd $DIR`).
local function cd(dir: string | false | nil, command: JouetCommand): string | false | nil
	local to = command.argv[2]
	if dir == false or not to or to == "-" or string.find(to, "^[~$]") then
		return false
	end
	if string.sub(to, 1, 1) == "/" or not dir then
		return to
	end
	return `{dir}/{to}`
end

local rule: JouetRule = {
	check = function(call: JouetCall): JouetDecision?
		local dir: string | false | nil = nil
		for _, command in call.commands do
			if command.program == "cd" and not command.nested then
				dir = cd(dir, command)
			else
				local kind, scope = classify(command, dir)
				if kind then
					-- Ignored when the command's own exit status is unknown.
					jouet.record(kind, command, if scope.full then "full" else "partial", scope.selector)
				end
			end
		end
		return nil
	end,
}

return rule

--!strict
-- The `piped-test-output` rule's script (piped-test-output.toml). Its
-- matcher lets through a call with a test or build runner and a program
-- that can cut output. The script fires when that cut is of the run's own
-- output: piped into head, tail, grep, rg or `sed -n` (through `tee` too),
-- or written to a file that a later command of the call reads with one of
-- them. Output written to a file and left there is not cut. Lints are
-- `scoped-lint`'s.

local function set(list: { string }): { [string]: boolean }
	local out = {}
	for _, item in list do
		out[item] = true
	end
	return out
end

local CUT = set({ "head", "tail", "grep", "egrep", "fgrep", "rg" })

-- Whether a command cuts what it reads.
local function cuts(command: JouetCommand): boolean
	local program = command.program or ""
	if CUT[program] then
		return true
	end
	if program ~= "sed" then
		return false
	end
	-- `sed -n` prints only what it is told to.
	for _, arg in jouet.options(table.move(command.argv, 2, #command.argv, 1, {})) do
		if arg == "--quiet" or arg == "--silent" or jouet.short_flag(arg, "n", "efl") then
			return true
		end
	end
	return false
end

-- A task runner's task that tests or builds, by its name's first part:
-- `test`, `test:e2e`, `build-docs`, `ci`.
local TASKS = set({ "test", "t", "e2e", "build", "b", "compile", "ci" })
local function task(name: string?): boolean
	return name ~= nil and TASKS[string.match(name, "^[^:%-_]*") or ""] == true
end

local CARGO = set({ "test", "t", "nextest", "build", "b", "check", "c" })
local RUNNERS = set({ "pytest", "vitest", "jest", "bats", "rspec", "tsc" })
-- Options that list or explain rather than run.
local INSPECT = set({
	"--help", "-h", "--version", "-V", "--list", "--collect-only", "--co", "--listTests",
	"--list-tests",
})

-- Whether a command runs tests or a build.
local function runs(command: JouetCommand): boolean
	local program = command.program or ""
	local args: { string } = table.move(command.argv, 2, #command.argv, 1, {})
	for _, arg in args do
		if INSPECT[arg] then
			return false
		end
	end
	-- `cargo +nightly test`: the toolchain comes first.
	if args[1] and string.sub(args[1], 1, 1) == "+" then
		table.remove(args, 1)
	end
	local first = args[1] or ""
	if program == "cargo" or program == "mbx" or program == "cross" then
		return CARGO[first] == true
	elseif program == "go" then
		return first == "test" or first == "build"
	elseif RUNNERS[program] then
		return true
	elseif program == "python" or program == "python3" then
		return first == "-m" and (args[2] == "pytest" or args[2] == "unittest")
	elseif program == "npx" or program == "bunx" then
		return RUNNERS[first] == true
	elseif program == "npm" or program == "pnpm" or program == "yarn" or program == "bun" then
		if first == "run" or first == "run-script" then
			return task(args[2])
		end
		return first == "test" or first == "t"
	elseif program == "mise" then
		return (first == "run" or first == "r") and task(args[2])
	elseif program == "make" then
		-- `make` alone builds the default target.
		return first == "" or string.sub(first, 1, 1) == "-" or task(first)
	elseif program == "just" or program == "task" then
		return task(first)
	elseif program == "dotnet" or program == "gradle" or program == "gradlew" or program == "mvn" then
		return first == "test" or first == "build" or first == "compile"
	end
	return false
end

-- A word as the shell reads it back.
local function quote(word: string): string
	if word ~= "" and not string.find(word, "[^%w%-_./:=+,@%%]") then
		return word
	end
	return "'" .. (string.gsub(word, "'", "'\\''")) .. "'"
end

-- The command that cuts `commands[i]`'s output, if one does.
local function cutter(commands: { JouetCommand }, i: number): JouetCommand?
	local files: { string } = {}
	if commands[i].stdout_to then
		table.insert(files, commands[i].stdout_to :: string)
	end
	-- Down its pipeline, where `tee FILE` also writes the output to a file.
	local j = i
	while commands[j].op == "pipe" and commands[j + 1] and not commands[j + 1].nested do
		j += 1
		local step = commands[j]
		if cuts(step) then
			return step
		end
		if step.program == "tee" then
			for _, arg in table.move(step.argv, 2, #step.argv, 1, {}) do
				if string.sub(arg, 1, 1) ~= "-" then
					table.insert(files, arg)
				end
			end
		end
	end
	-- A file the output went to, read back later in the call.
	for k = j + 1, #commands do
		local later = commands[k]
		if not later.nested and cuts(later) then
			for _, file in files do
				if jouet.contains(later.argv, file) then
					return later
				end
			end
		end
	end
	return nil
end

local rule: JouetRule = {
	check = function(call: JouetCall): JouetDecision?
		for i, command in call.commands do
			if command.nested or not runs(command) then
				continue
			end
			local cut = cutter(call.commands, i)
			if cut then
				local words = {}
				for _, word in command.words do
					table.insert(words, quote(word))
				end
				return jouet.warn(
					`Run \`jouet test --cmd -- {table.concat(words, " ")}\` instead of cutting its output with {cut.program}. It keeps the whole output in a log, and prints one line of counts when the run passes, or each failure with the lines that explain it (up to --budget bytes) and the log's path, to read more from rather than running it again.`
				)
			end
		end
		return nil
	end,
}

return rule

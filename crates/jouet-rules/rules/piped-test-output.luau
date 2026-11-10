--!strict
-- The `piped-test-output` rule's script (piped-test-output.toml). Its
-- matcher lets through a call with a test or build runner and a program
-- that can cut output. The script fires when that cut is of the run's own
-- output: piped into head, tail, grep, rg or `sed -n` (through `tee` too),
-- or written to a file that a later command of the call reads with one of
-- them. Output written to a file and left there is not cut. Lints are
-- `scoped-lint`'s. Where the harness lets it (`call.rewrite`), a run piped
-- straight into the programs that cut it runs under `jouet test --cmd`
-- instead, uncut; the rest of the call stays as written.

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

-- A word as the shell reads it back. `$VAR`, globs and `~` stay bare, so
-- the shell expands them as it would have in the original command, since
-- `--cmd` runs without a shell. Any other word is double-quoted, which
-- still expands `$`.
local function quote(word: string): string
	if word ~= "" and not string.find(word, "[^%w%-_./:=+,@%%$*?%[%]~{}]") then
		return word
	end
	return '"' .. (string.gsub(word, '[\\"`]', "\\%0")) .. '"'
end

-- A leading `NAME=value` assignment, with its value quoted as a word.
local function assignment(text: string): string
	local name, value = string.match(text, "^([^=]*)=(.*)$")
	return `{name}={if value == "" then '""' else quote(value :: string)}`
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

-- grep and rg options that answer with a count, file names or an exit
-- status rather than lines of the run; a rewrite would lose that answer.
local ANSWERS = set({
	"--count", "--count-matches", "--quiet", "--silent", "--files-with-matches",
	"--files-without-match",
})
local function answers(command: JouetCommand): boolean
	local program = command.program or ""
	if program == "head" or program == "tail" or program == "sed" then
		return false
	end
	for _, arg in jouet.options(table.move(command.argv, 2, #command.argv, 1, {})) do
		if ANSWERS[arg] then
			return true
		end
		for _, flag in { "q", "c", "l", "L" } do
			if jouet.short_flag(arg, flag, "efABCmdDgtT") then
				return true
			end
		end
	end
	return false
end

-- The last command of the pipeline that cuts `commands[i]`'s output, when
-- the run can go under `jouet test --cmd` instead: it is piped straight
-- into programs that only cut it, reads no input of its own and writes no
-- file, and the pipeline ends the call or a step of it in the foreground.
-- nil when the call must be warned about or denied as written.
local function pipeline_end(commands: { JouetCommand }, i: number): JouetCommand?
	local command = commands[i]
	if not command.span or not command.words_at or command.stdout_to or command.stdin_redirected then
		return nil
	end
	if i > 1 and commands[i - 1].op == "pipe" then
		return nil
	end
	local j = i
	while commands[j].op == "pipe" do
		local step = commands[j + 1]
		if not step or step.nested or not step.span or not cuts(step) or answers(step) or step.stdin_redirected then
			return nil
		end
		j += 1
	end
	local last = commands[j]
	if j == i or last.stdout_to or last.op == "background" then
		return nil
	end
	return last
end

-- Commands that run a project's whole test suite, which plain `jouet test`
-- runs too, followed only by words that do not select tests.
local SUITES: { { string } } = {
	{ "cargo", "test" },
	{ "cargo", "nextest", "run" },
	{ "python", "-m", "unittest" },
	{ "python3", "-m", "unittest" },
	{ "python", "-m", "pytest" },
	{ "python3", "-m", "pytest" },
	{ "pytest" },
	{ "go", "test", "./..." },
	{ "npm", "test" },
	{ "pnpm", "test" },
	{ "yarn", "test" },
	{ "mise", "run", "test" },
	{ "just", "test" },
	{ "make", "test" },
}
local UNSELECTIVE = set({ "-q", "--quiet", "-v", "--verbose", "--workspace", "discover" })

-- Whether `command` is the whole suite: then `jouet test` is the simpler
-- suggestion, and what it records is a full run.
local function whole_suite(command: JouetCommand): boolean
	local words = command.argv
	for _, suite in SUITES do
		local matches = #words >= #suite
		for k, word in suite do
			if words[k] ~= word then
				matches = false
				break
			end
		end
		if matches then
			for k = #suite + 1, #words do
				if not UNSELECTIVE[words[k]] then
					return false
				end
			end
			return true
		end
	end
	return false
end

-- Whether a command is a `jouet test` run, past jouet's own options; a
-- dry run or help only asks about one.
local function jouet_test(command: JouetCommand): boolean
	if command.program ~= "jouet" then
		return false
	end
	local i = 2
	while command.argv[i] do
		local arg = command.argv[i]
		if arg == "--display" then
			i += 2
		elseif string.sub(arg, 1, 1) == "-" then
			i += 1
		elseif arg ~= "test" then
			return false
		else
			for k = i + 1, #command.argv do
				local word = command.argv[k]
				if word == "--" then
					break
				elseif word == "--dry-run" or INSPECT[word] then
					return false
				end
			end
			return true
		end
	end
	return false
end

-- The script of a `jouet test --cmd -- sh -c SCRIPT`.
local SHELLS = set({ "sh", "bash", "zsh", "dash", "ksh" })
local function script_under(command: JouetCommand): string?
	local argv = command.argv
	local at = table.find(argv, "--")
	if not at or not SHELLS[jouet.basename(argv[at + 1] or "")] then
		return nil
	end
	for k = at + 2, #argv do
		local arg = argv[k]
		if string.sub(arg, 1, 1) ~= "-" then
			return nil
		end
		if string.find(string.sub(arg, 2), "c", 1, true) and string.sub(arg, 1, 2) ~= "--" then
			return argv[k + 1]
		end
	end
	return nil
end

-- The rule warns the first time in a session, and denies after that: a
-- warning alone was followed once and then forgotten, and the cut run was
-- often run again to see what the cut dropped. A subagent was not shown its
-- session's warning, so it gets its own.
local function speak(call: JouetCall, message: string): JouetDecision
	if call.session then
		local fired = jouet.session.fired("piped-test-output", call.session, call.agent)
		if fired and fired > 0 then
			return jouet.deny(`This session was already warned about cutting a run's output. {message} Nothing in this call ran, so run its other commands again too.`)
		end
	end
	return jouet.warn(message)
end

-- One run to put under `jouet test --cmd`: where its words start and end
-- in the call's command, and where the pipeline that cut it ends.
type Swap = { at: number, run: number, cut: number, by: string }

-- The call's command with each run under `jouet test --cmd` and the
-- programs that cut it gone.
local function swapped(command: string, swaps: { Swap }): string
	-- From the right, so the positions of the runs before stay true.
	for k = #swaps, 1, -1 do
		local s = swaps[k]
		command = string.sub(command, 1, s.at - 1)
			.. "jouet test --cmd -- "
			.. string.sub(command, s.at, s.run)
			.. string.sub(command, s.cut + 1)
	end
	return command
end

local rule: JouetRule = {
	check = function(call: JouetCall): JouetDecision?
		local swaps: { Swap } = {}
		for i, command in call.commands do
			if command.nested then
				continue
			end
			-- `jouet test` already prints only what matters, keeps the whole
			-- output in its log, and records its own result for the hooks,
			-- piped or not. Inside it, a shell script that cuts a run leaves
			-- it only the cut output to read, and the exit code of the
			-- script's last command.
			if jouet_test(command) then
				-- Its own output piped into head or tail is short already, but a
				-- failure is longer than a pass: the cut can drop the line naming
				-- what failed, and its exit code, so a `&&` after it goes on.
				-- That is said once, as a warning: most such runs pass.
				-- Down its pipeline, past `tee` or another filter.
				local cut_by: JouetCommand? = nil
				local j = i
				while call.commands[j].op == "pipe" and call.commands[j + 1] and not call.commands[j + 1].nested do
					j += 1
					local step = call.commands[j]
					if step.program == "tail" or step.program == "head" then
						cut_by = step
						break
					end
				end
				if cut_by then
					local fired = if call.session
						then jouet.session.fired("piped-test-output", call.session, call.agent)
						else nil
					if not fired or fired == 0 then
						return jouet.warn(
							`\`jouet test\` keeps a passing run to one line and a failure within --budget bytes (default 4000); {cut_by.program} can cut the line naming the failure, and the pipe hides its exit code from \`&&\`. Run it without the pipe, with \`--budget BYTES\` for a shorter failure.`
						)
					end
				end
				local script = script_under(command)
				if script then
					local inner = jouet.parse(script)
					for j, step in inner do
						local cut: JouetCommand? = nil
						if not step.nested and runs(step) then
							cut = cutter(inner, j)
						end
						if cut then
							local env, words = {}, {}
							for _, text in step.env do
								table.insert(env, assignment(text) .. " ")
							end
							for _, word in step.words do
								table.insert(words, quote(word))
							end
							return speak(
								call,
								`\`jouet test\` reads only what this script prints after {cut.program} cut it, and the exit code of its last command. Run \`{table.concat(env)}jouet test --cmd -- {table.concat(words, " ")}\` on its own for each run instead, uncut: it keeps the whole output in a log and prints each failure, or one line of counts when the run passes.`
							)
						end
					end
				end
				continue
			end
			if not runs(command) then
				continue
			end
			local cut = cutter(call.commands, i)
			local last = if cut and call.rewrite and call.command then pipeline_end(call.commands, i) else nil
			if last then
				table.insert(swaps, {
					at = command.words_at :: number,
					run = (command.span :: { first: number, last: number }).last,
					cut = (last.span :: { first: number, last: number }).last,
					by = (cut :: JouetCommand).program or "",
				})
			elseif cut then
				-- Assignments go before jouet, whose environment the command
				-- inherits.
				local env = {}
				for _, text in command.env do
					table.insert(env, assignment(text) .. " ")
				end
				local run = "jouet test"
				if not whole_suite(command) then
					local words = {}
					for _, word in command.words do
						table.insert(words, quote(word))
					end
					run = `jouet test --cmd -- {table.concat(words, " ")}`
				end
				return speak(
					call,
					`Run \`{table.concat(env)}{run}\` instead of cutting its output with {cut.program}. It keeps the whole output in a log, and prints one line of counts when the run passes, or each failure with the lines that explain it (up to --budget bytes) and the log's path, to read more from rather than running it again.`
				)
			end
		end
		if #swaps > 0 then
			local command = swapped(call.command :: string, swaps)
			return jouet.rewrite(
				command,
				`jouet ran this call as \`{command}\`, on purpose: a test or build run cut with {swaps[1].by} goes under \`jouet test --cmd\` instead, which keeps the whole output in a log and prints one line of counts when the run passes, or each failure with the lines that explain it and the log's path, to read rather than running it again.`
			)
		end
		return nil
	end,
}

return rule

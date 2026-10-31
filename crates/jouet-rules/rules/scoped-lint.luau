--!strict
-- The `scoped-lint` rule's script (scoped-lint.toml). Its matcher only lets
-- a lint of the whole project through. The script stays quiet when that lint
-- is the full check the message asks for at the end, not iterating:
-- - the work tree has nothing uncommitted. Then there is no change to iterate
--   on: `jouet diag check --changed` would check nothing, and a whole-tree
--   lint of a commit, such as after a merge, pull or checkout, is that check;
-- - clippy runs as CI does, denying warnings across the workspace or all
--   targets (`cargo clippy --workspace --all-targets -- -D warnings`);
-- - the same call also runs the test suite: a check before pushing.

-- Whether `argv` denies warnings: `-D warnings`, `-Dwarnings` or
-- `--deny warnings`, anywhere, as clippy takes them after `--`.
local function denies_warnings(argv: { string }): boolean
	for i, arg in argv do
		if arg == "-Dwarnings" or arg == "--deny=warnings" then
			return true
		end
		if (arg == "-D" or arg == "--deny") and argv[i + 1] == "warnings" then
			return true
		end
	end
	return false
end

-- Whether a command is clippy the way CI runs it.
local function full_clippy(command: JouetCommand): boolean
	local argv = command.argv
	if command.program ~= "cargo" or argv[2] ~= "clippy" then
		return false
	end
	local opts = jouet.options(argv)
	local wide = jouet.contains(opts, "--workspace")
		or jouet.contains(opts, "--all")
		or jouet.contains(opts, "--all-targets")
	return wide and denies_warnings(argv)
end

-- Whether a command runs a test suite.
local function runs_tests(command: JouetCommand): boolean
	local argv = command.argv
	local program = command.program
	if program == "cargo" then
		return argv[2] == "test" or argv[2] == "nextest"
	end
	if program == "mise" or program == "npm" or program == "pnpm" or program == "yarn" or program == "just" or program == "make" then
		for i = 2, #argv do
			local word = argv[i]
			if word == "test" or string.sub(word, 1, 5) == "test:" then
				return true
			end
		end
		return false
	end
	if program == "jouet" then
		return argv[2] == "test"
	end
	return program == "bats" or program == "pytest" or program == "vitest" or program == "jest"
end

local rule: JouetRule = {
	check = function(call: JouetCall): JouetDecision?
		for _, command in call.commands do
			if full_clippy(command) or runs_tests(command) then
				return nil
			end
		end
		-- Without the probe (as in most of the rule's examples) the state is
		-- unknown, and the rule fires on what the matcher found.
		local state = jouet.git.state()
		if state and not state.dirty then
			return nil
		end
		return jouet.warn(
			"Run `jouet diag check --changed` instead. It checks only the files you changed, through the project's hk checks, and prints one line per finding (`jouet diag delta` shows only what your edit introduced). Run the full lint once before you finish."
		)
	end,
}

return rule

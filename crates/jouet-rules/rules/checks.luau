--!strict
-- The `checks` rule's script (checks.toml): which test, lint and build runs
-- to record. It is a script because it reads task names (`mise run
-- test:e2e`, but not `lint-fix`) and each tool's subcommands, and records
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

local function classify(command: JouetCommand): Kind?
	local program: string = command.program or ""
	local args: { string } = table.move(command.argv, 2, #command.argv, 1, {})
	-- `cargo +nightly test`: the toolchain comes first.
	if args[1] and string.sub(args[1], 1, 1) == "+" then
		table.remove(args, 1)
	end
	local first: string? = args[1]
	if program == "cargo" or program == "mbx" or program == "cross" then
		if first == "fmt" then
			return if jouet.contains(args, "--check") then "lint" else nil
		end
		return if first then CARGO[first] :: Kind? else nil
	elseif program == "mise" and (first == "run" or first == "r") then
		return task(args[2])
	elseif program == "npm" or program == "pnpm" or program == "yarn" or program == "bun" or program == "aube" then
		if first == "run" or first == "run-script" then
			return task(args[2])
		end
		return if first == "test" or first == "t" then "test" else nil
	elseif program == "make" or program == "just" or program == "task" then
		return task(first)
	elseif program == "go" then
		return if first == "test" then "test" elseif first == "vet" then "lint" elseif first == "build" then "build" else nil
	elseif program == "hk" then
		return if first == "check" or first == "run" then "lint" else nil
	elseif program == "pytest" or program == "vitest" or program == "jest" or program == "bats" or program == "rspec" then
		return "test"
	elseif (program == "python" or program == "python3") and first == "-m" and args[2] == "pytest" then
		return "test"
	elseif program == "ruff" then
		return if first == "check" then "lint" else nil
	elseif program == "eslint" or program == "mypy" or program == "shellcheck" or program == "actionlint" then
		return "lint"
	elseif program == "tsc" then
		return "build"
	elseif program == "dotnet" or program == "swift" or program == "gradle" or program == "gradlew" or program == "mvn" then
		return if first == "test" then "test" elseif first == "build" or first == "compile" then "build" else nil
	end
	return nil
end

local rule: JouetRule = {
	check = function(call: JouetCall): JouetDecision?
		for _, command in call.commands do
			local kind = classify(command)
			if kind then
				-- Ignored when the command's own exit status is unknown.
				jouet.record(kind, command)
			end
		end
		return nil
	end,
}

return rule

--!strict
--- name: checks
--- why: Records test, lint and build runs, so jouet can stop an agent finishing while the last one failed.
--- scope: builtin
--- author: builtin
--- created: 2026-10-30

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
	on = "post-tool-use",
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
	examples = {
		record = {
			{ command = "cargo test --all-targets", exit_code = 101, kind = "test" },
			{ command = "rtk cargo test", exit_code = 101, kind = "test" },
			{ command = "cd crate && cargo test", exit_code = 101, kind = "test" },
			{ command = "cargo +1.91 check --locked", exit_code = 0, kind = "build" },
			{ command = "cargo clippy -- -D warnings", exit_code = 101, kind = "lint" },
			{ command = "cargo fmt --check", exit_code = 1, kind = "lint" },
			{ command = "mise run test:e2e edit", exit_code = 0, kind = "test" },
			{ command = "mise run lint", exit_code = 1, kind = "lint" },
			{ command = "npm run build", exit_code = 0, kind = "build" },
			{ command = "pnpm test", exit_code = 0, kind = "test" },
			{ command = "python3 -m pytest -x", exit_code = 1, kind = "test" },
			{ command = "go vet ./...", exit_code = 0, kind = "lint" },
			{ command = "make check", exit_code = 0, kind = "lint" },
			{ command = "set -o pipefail; cargo test | tee log", exit_code = 101, kind = "test" },
		},
		allow = {
			{ command = "cargo fmt", exit_code = 0 },
			{ command = "mise run lint-fix", exit_code = 0 },
			{ command = "cargo run", exit_code = 0 },
			{ command = "npm install", exit_code = 0 },
			{ command = "cargo test; echo done", exit_code = 1 },
			{ command = "cargo test || true", exit_code = 0 },
			{ command = "cargo test 2>&1 | tail -5", exit_code = 0 },
			{ command = "cargo test &", exit_code = 0 },
		},
	},
}

return rule

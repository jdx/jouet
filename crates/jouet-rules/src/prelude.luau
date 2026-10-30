-- The parts of the `jouet` module written in Luau. Runs once per VM, before
-- the sandbox makes the module read-only, with the module table as its
-- argument. Rules see these exactly as they see the functions written in Rust.

local jouet = ...

function jouet.deny(message: string)
	return { action = "deny", message = message }
end

function jouet.warn(message: string)
	return { action = "warn", message = message }
end

function jouet.allow()
	return { action = "allow" }
end

function jouet.basename(path: string): string
	local name = string.match(path, "[^/\\]*$") or path
	return (string.gsub(name, "%.exe$", ""))
end

function jouet.contains(list: { string }, value: string): boolean
	return table.find(list, value) ~= nil
end

-- The arguments before a `--` that ends the options.
function jouet.options(args: { string }): { string }
	local out = {}
	for _, arg in args do
		if arg == "--" then
			break
		end
		table.insert(out, arg)
	end
	return out
end

-- Whether a short-option cluster such as `-fu` holds `flag`. Stops at the
-- first letter in `takes_value`, whose value is the rest of the word.
function jouet.short_flag(arg: string, flag: string, takes_value: string?): boolean
	if #arg < 2 or string.sub(arg, 1, 1) ~= "-" or string.sub(arg, 2, 2) == "-" then
		return false
	end
	for i = 2, #arg do
		local c = string.sub(arg, i, i)
		if c == flag then
			return true
		end
		if takes_value and string.find(takes_value, c, 1, true) then
			return false
		end
	end
	return false
end

-- git's options that take the next word as their value.
local GIT_VALUE_OPTIONS = {
	["-C"] = true,
	["-c"] = true,
	["--git-dir"] = true,
	["--work-tree"] = true,
	["--namespace"] = true,
	["--exec-path"] = true,
}

-- A git command's subcommand and its arguments, past global options such as
-- `-C dir`; nil when the command is not git. `jouet.git` is a table, which
-- also holds the `state` probe, called as a function through `__call`.
local function git_subcommand(command): (string?, { string })
	if command.program ~= "git" then
		return nil, {}
	end
	local argv = command.argv
	local i = 2
	while i <= #argv do
		local arg = argv[i]
		if GIT_VALUE_OPTIONS[arg] then
			i += 2
		elseif string.sub(arg, 1, 1) == "-" then
			i += 1
		else
			return arg, table.move(argv, i + 1, #argv, 1, {})
		end
	end
	return nil, {}
end

setmetatable(jouet.git, {
	__call = function(_, command)
		return git_subcommand(command)
	end,
})

-- A gh command's arguments after `gh`; nil when the command is not gh.
function jouet.gh(command): { string }?
	if command.program ~= "gh" then
		return nil
	end
	return table.move(command.argv, 2, #command.argv, 1, {})
end

--!strict
-- The `claimed-path` rule's script (claimed-path.toml). The matcher lets an
-- edit or a commit through; the script works out the paths it touches and
-- asks `jouet claim`'s store whether another live session claims any.

-- Options that take a value, so the value is not a path.
local GIT_COMMIT_VALUES = {
	"-m",
	"--message",
	"-F",
	"--file",
	"-C",
	"--reuse-message",
	"-c",
	"--reedit-message",
	"--author",
	"--date",
	"--cleanup",
	"-t",
	"--template",
	"--fixup",
	"--squash",
	"--trailer",
	"--pathspec-from-file",
}
local GIT_COMMIT_SHORT_VALUES = "mFCct"
local JOUET_VALUES =
	{ "-m", "--message", "-F", "--file", "--trailer", "--old", "--new", "--count", "--check", "--expect-hash" }
local JOUET_SHORT_VALUES = "mF"

-- The arguments that are not options or their values.
local function positional(args: { string }, long: { string }, short: string): { string }
	local out = {}
	local skip, ended = false, false
	for _, arg in args do
		if skip then
			skip = false
		elseif ended or string.sub(arg, 1, 1) ~= "-" or arg == "-" then
			table.insert(out, arg)
		elseif arg == "--" then
			ended = true
		elseif jouet.contains(long, arg) then
			skip = true
		elseif string.sub(arg, 2, 2) ~= "-" then
			-- A short cluster such as `-am`: a value letter at its end takes
			-- the next argument.
			local last = string.sub(arg, -1)
			local first_value = string.find(arg, `[{short}]`, 2)
			skip = first_value ~= nil and first_value == #arg and string.find(short, last, 1, true) ~= nil
		end
	end
	return out
end

-- The paths a command touches, or nil when it touches none jouet can name.
local function paths_of(command: JouetCommand): { string }?
	local sub, args = jouet.git(command)
	if sub == "commit" then
		local named = positional(args, GIT_COMMIT_VALUES, GIT_COMMIT_SHORT_VALUES)
		if #named > 0 then
			return named
		end
		local all = false
		for _, arg in jouet.options(args) do
			if arg == "--all" or jouet.short_flag(arg, "a", GIT_COMMIT_SHORT_VALUES) then
				all = true
			end
		end
		local changed = jouet.git.changed(if all then "tracked" else "staged")
		return changed
	end
	local argv = command.argv
	if command.program == "jouet" and (argv[2] == "commit" or (argv[2] == "edit" and argv[3] == "replace")) then
		local rest = {}
		for i = if argv[2] == "commit" then 3 else 4, #argv do
			table.insert(rest, argv[i])
		end
		-- `edit replace` takes several files, like `commit`.
		return positional(rest, JOUET_VALUES, JOUET_SHORT_VALUES)
	end
	return nil
end

local rule: JouetRule = {
	check = function(call: JouetCall): JouetDecision?
		local paths: { string } = {}
		if call.path then
			table.insert(paths, call.path)
		end
		for _, command in call.commands do
			local found: { string } = paths_of(command) or {}
			for _, path in found do
				table.insert(paths, path)
			end
		end
		if #paths == 0 then
			return nil
		end
		local held = jouet.claims.held(paths)
		if not held then
			return nil
		end
		local lines, seen = {}, {}
		for _, claim in held do
			local key = `{claim.session}\0{claim.pattern}`
			if claim.session ~= call.session and not seen[key] then
				seen[key] = true
				local where = claim.worktree
				if claim.branch then
					where ..= ` (branch {claim.branch})`
				end
				local line = `{claim.path} is claimed as {claim.pattern} by session {string.sub(claim.session, 1, 8)} in {where}, until {claim.expires_at}`
				if claim.note then
					line ..= `: {claim.note}`
				end
				table.insert(lines, line)
			end
		end
		if #lines == 0 then
			return nil
		end
		return jouet.warn(
			`Another session is working on this: {table.concat(lines, "; ")}. Leave it to that session or coordinate first; \`jouet claim ls\` lists the claims.`
		)
	end,
}

return rule

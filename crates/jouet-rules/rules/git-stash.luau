--!strict
--- name: git-stash
--- why: Every worktree of a repository shares one stash, so a bare stash, pop or drop can take another session's work.
--- mode: deny
--- scope: builtin
--- author: builtin
--- created: 2026-10-30

local MESSAGE =
	"Every worktree of a repository shares one stash, so a bare stash, pop or drop can take another session's work: commit a WIP instead, or `git stash push -m <unique-tag>` and restore with `git stash apply <sha>`."

local function has_message(args: { string }): boolean
	for _, arg in jouet.options(args) do
		if
			arg == "--message"
			or string.sub(arg, 1, 10) == "--message="
			or (string.sub(arg, 1, 2) == "-m")
			or jouet.short_flag(arg, "m")
		then
			return true
		end
	end
	return false
end

local function has_positional(args: { string }): boolean
	for _, arg in args do
		if string.sub(arg, 1, 1) ~= "-" then
			return true
		end
	end
	return false
end

local function risky(args: { string }): boolean
	local sub = args[1]
	local rest = table.move(args, 2, #args, 1, {})
	if sub == nil then
		return true
	elseif string.sub(sub, 1, 1) == "-" then
		return not has_message(args)
	elseif sub == "push" then
		return not has_message(rest)
	elseif sub == "save" then
		return not has_positional(rest) and not has_message(rest)
	elseif sub == "pop" or sub == "apply" or sub == "drop" then
		-- Without a stash named, these act on whichever entry is on top.
		return not has_positional(rest)
	elseif sub == "branch" then
		-- `git stash branch <name>` applies and drops the top entry.
		local positional = 0
		for _, arg in rest do
			if string.sub(arg, 1, 1) ~= "-" then
				positional += 1
			end
		end
		return positional < 2
	elseif sub == "clear" then
		return true
	end
	return false
end

local rule: JouetRule = {
	check = function(call: JouetCall): JouetDecision?
		for _, command in call.commands do
			local sub, args = jouet.git(command)
			if sub == "stash" and risky(args) then
				return jouet.deny(MESSAGE)
			end
		end
		return nil
	end,
	examples = {
		deny = {
			"git stash",
			"git stash -u",
			"git stash push",
			"git stash push -u src/",
			"git stash pop",
			"git stash apply",
			"git stash drop",
			"git stash clear",
			"git stash save",
			"git stash branch rescue",
		},
		allow = {
			"git stash push -m wip-ab12",
			"git stash push -u -m 'wip ab12'",
			"git stash push --message=wip",
			"git stash -u -m wip",
			"git stash save 'wip ab12'",
			"git stash list",
			"git stash show -p stash@{1}",
			"git stash apply 3f2a9c1",
			"git stash drop stash@{2}",
			"git stash branch rescue stash@{1}",
			"echo 'git stash'",
			"git commit -m 'avoid git stash'",
		},
	},
}

return rule

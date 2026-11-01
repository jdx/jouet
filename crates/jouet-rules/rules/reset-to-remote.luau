--!strict
-- The `reset-to-remote` rule's script (reset-to-remote.toml). The matcher
-- finds a command that points a branch at another commit; the script asks
-- how many of the branch's own commits that commit lacks.

-- Options of `git checkout`/`switch`/`branch`/`reset` that take a value.
local VALUES = { "--track", "-t", "--conflict", "--pathspec-from-file", "--orphan" }

-- The arguments that are not options, and whether `--` ended them.
local function positional(args: { string }): { string }
	local out = {}
	local skip = false
	for _, arg in args do
		if skip then
			skip = false
		elseif arg == "--" then
			break
		elseif string.sub(arg, 1, 1) == "-" then
			skip = jouet.contains(VALUES, arg)
		else
			table.insert(out, arg)
		end
	end
	return out
end

-- The branch a command would move and where to, or nil. `dir` is where a
-- `cd` before it went (see `jouet.dirs`), nil for the call's directory.
local function target(command: JouetCommand, dir: string?): (string?, string?)
	local sub, args = jouet.git(command)
	local named = positional(args)
	if sub == "checkout" or sub == "switch" or sub == "branch" then
		-- `-B X START`, `-C X START`, `branch -f X START`: without a start
		-- point the branch moves to HEAD, which it is about to be anyway.
		return named[1], named[2]
	elseif sub == "reset" then
		-- Only a reset to a remote's branch: `git reset --hard HEAD^` is a
		-- deliberate step back, not a restack.
		local start = named[1]
		if start and (string.match(start, "^origin/") or string.match(start, "^upstream/")) then
			local state = jouet.git.state(dir)
			return state and state.branch, start
		end
	end
	return nil, nil
end

local rule: JouetRule = {
	check = function(call: JouetCall): JouetDecision?
		local dirs = jouet.dirs(call.commands)
		for i, command in call.commands do
			-- After a `cd` it cannot follow, the repository is unknown, and
			-- the call's own would be the wrong one to ask.
			local dir = dirs[i]
			local branch: string?, start: string? = nil, nil
			if dir ~= false then
				branch, start = target(command, dir)
			end
			if branch and start then
				local lost = jouet.git.ahead(branch, start, dir or nil)
				if lost and lost > 0 then
					local s = if lost == 1 then "" else "s"
					return jouet.warn(
						`This drops {lost} commit{s} on the local \`{branch}\` that \`{start}\` does not have, such as fixes not pushed yet. Keep them by merging instead: \`git merge {start}\` on \`{branch}\`, or \`jouet stack sync\` for a stack of PRs.`
					)
				end
			end
		end
		return nil
	end,
}

return rule

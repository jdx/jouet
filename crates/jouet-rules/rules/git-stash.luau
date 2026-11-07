--!strict
-- The `git-stash` rule's script (git-stash.toml). The matcher finds a stash
-- command that takes whichever entry is on top, or pushes one nobody can
-- tell apart; the script lets it through in a repository with one worktree,
-- whose stash nothing else shares.

local MESSAGE =
	"Every worktree of a repository shares one stash, so a bare stash, pop or drop can take another session's work: commit a WIP instead, or `git stash push -m <unique-tag>` and restore with `git stash apply <sha>`."

-- Whether a git command names its repository itself (`git -C DIR`,
-- `--git-dir`, `--work-tree`, or `GIT_DIR`/`GIT_WORK_TREE` set for it),
-- which `jouet.repo()`, asking about the call's own, does not see.
local function elsewhere(command: JouetCommand): boolean
	for _, text in command.env do
		if string.match(text, "^GIT_DIR=") or string.match(text, "^GIT_WORK_TREE=") then
			return true
		end
	end
	for k = 2, #command.argv do
		local arg = command.argv[k]
		if arg == "stash" then
			break
		end
		if
			arg == "-C"
			or arg == "--git-dir"
			or arg == "--work-tree"
			or string.match(arg, "^%-%-git%-dir=")
			or string.match(arg, "^%-%-work%-tree=")
		then
			return true
		end
	end
	return false
end

local rule: JouetRule = {
	check = function(call: JouetCall): JouetDecision?
		-- After a `cd`, the repository may be another one; without an
		-- answer, it may have worktrees.
		for _, dir in jouet.dirs(call.commands) do
			if dir ~= nil then
				return jouet.deny(MESSAGE)
			end
		end
		for _, command in call.commands do
			if command.program == "git" and elsewhere(command) then
				return jouet.deny(MESSAGE)
			end
		end
		local repo = jouet.repo()
		if repo and repo.worktrees == 1 then
			return nil
		end
		return jouet.deny(MESSAGE)
	end,
}

return rule

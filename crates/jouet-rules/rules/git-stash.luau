--!strict
-- The `git-stash` rule's script (git-stash.toml). The matcher finds a stash
-- command that takes whichever entry is on top, or pushes one nobody can
-- tell apart; the script lets it through in a repository with one worktree,
-- whose stash nothing else shares.

local MESSAGE =
	"Every worktree of a repository shares one stash, so a bare stash, pop or drop can take another session's work: commit a WIP instead, or `git stash push -m <unique-tag>` and restore with `git stash apply <sha>`."

-- Whether a stash runs in the call's own repository, the one `jouet.repo()`
-- answers about: a plain `git stash ...`, with nothing before `stash` and no
-- wrapper or variable that could name another (`git -C DIR`, `--git-dir`,
-- `env GIT_DIR=...`). Anything else may reach a repository with worktrees.
local function here(command: JouetCommand): boolean
	return command.words[1] == "git"
		and command.argv[1] == "git"
		and command.argv[2] == "stash"
		and #command.via == 0
		and #command.env == 0
end

local rule: JouetRule = {
	check = function(call: JouetCall): JouetDecision?
		-- After a `cd`, the repository may be another one.
		for _, dir in jouet.dirs(call.commands) do
			if dir ~= nil then
				return jouet.deny(MESSAGE)
			end
		end
		for _, command in call.commands do
			if command.matched and not here(command) then
				return jouet.deny(MESSAGE)
			end
		end
		-- Without an answer, the repository may have worktrees.
		local repo = jouet.repo()
		if repo and repo.worktrees == 1 then
			return nil
		end
		return jouet.deny(MESSAGE)
	end,
}

return rule

--!strict
-- The `git-stash` rule's script (git-stash.toml). The matcher finds a stash
-- command that takes whichever entry is on top, or pushes one nobody can
-- tell apart; the script lets it through in a repository with one worktree,
-- whose stash nothing else shares.

local MESSAGE =
	"Every worktree of a repository shares one stash, so a bare stash, pop or drop can take another session's work: commit a WIP instead, or `git stash push -m <unique-tag>` and restore with `git stash apply <sha>`."

local rule: JouetRule = {
	check = function(call: JouetCall): JouetDecision?
		-- After a `cd`, the repository may be another one; without an
		-- answer, it may have worktrees.
		for _, dir in jouet.dirs(call.commands) do
			if dir ~= nil then
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

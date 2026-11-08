--!strict
-- The `git-stash` rule's script (git-stash.toml). The matcher finds a stash
-- command that takes whichever entry is on top, or pushes one nobody can
-- tell apart; the script lets it through in a repository with one worktree,
-- whose stash nothing else shares.

local MESSAGE =
	"Every worktree of a repository shares one stash, so a bare stash, pop or drop can take another session's work: commit a WIP instead, or `git stash push -m <unique-tag>` and restore with `git stash apply <sha>`."

-- Whether a stash runs in the repository of the directory it runs in, the
-- one `jouet.repo` answers about: a plain `git stash ...`, with nothing
-- before `stash` and no wrapper or variable that could name another
-- (`git -C DIR`, `--git-dir`, `env GIT_DIR=...`).
local function here(command: JouetCommand): boolean
	return command.words[1] == "git"
		and command.argv[1] == "git"
		and command.argv[2] == "stash"
		and #command.via == 0
		and #command.env == 0
end

local rule: JouetRule = {
	check = function(call: JouetCall): JouetDecision?
		local dirs = jouet.dirs(call.commands)
		for i, command in call.commands do
			if command.matched then
				-- A `cd` that cannot be followed may lead anywhere.
				local dir = dirs[i]
				if dir == false or not here(command) then
					return jouet.deny(MESSAGE)
				end
				-- Without an answer, the repository may have worktrees.
				local repo = jouet.repo(dir or nil)
				if not repo or repo.worktrees ~= 1 then
					return jouet.deny(MESSAGE)
				end
			end
		end
		return nil
	end,
}

return rule

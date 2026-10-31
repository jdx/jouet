--!strict
-- The `scoped-lint` rule's script (scoped-lint.toml). Its matcher only lets
-- a lint of the whole project through; the script stays quiet when the work
-- tree has nothing uncommitted. Then there is no change to iterate on:
-- `jouet diag check --changed` would check nothing, and a whole-tree lint of
-- a commit, such as after a merge, pull or checkout, is the full lint the
-- message asks for at the end.

local rule: JouetRule = {
	check = function(_call: JouetCall): JouetDecision?
		-- Without the probe (as in most of the rule's examples) the state is
		-- unknown, and the rule fires on what the matcher found.
		local state = jouet.git.state()
		if state and not state.dirty then
			return nil
		end
		return jouet.warn(
			"Run `jouet diag check --changed` instead. It checks only the files you changed, through the project's hk checks, and prints one line per finding (`jouet diag delta` shows only what your edit introduced). Run the full lint once before you finish."
		)
	end,
}

return rule

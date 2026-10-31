--!strict
-- The `push-untested` rule's script (push-untested.toml). Its matcher only
-- lets a push through, so the script asks whether the work tree is known to
-- pass the full test suite.

-- Why the work tree at `dir` (the call's own when nil) is not known to pass
-- the full suite; nil when it is, or when nothing was ever recorded there to
-- judge by (the ledger is off, or jouet's hooks have not seen a check in
-- that work tree).
local function untested(dir: string?): string?
	local any, err = jouet.ci.last_run(nil, nil, dir)
	if any == nil and err == nil then
		return nil
	end
	local full = jouet.ci.last_run("test", "full", dir)
	local tree = jouet.git.tree(dir)
	if full and full.ok and tree and full.tree == tree then
		return nil
	end
	if not full then
		return "no full test run is recorded in this work tree"
	elseif not full.ok then
		return "the last full test run failed"
	elseif tree and full.tree then
		return "the tree has changed since the last full test run passed"
	end
	return "jouet cannot tell whether the last full test run saw this tree"
end

local rule: JouetRule = {
	check = function(call: JouetCall): JouetDecision?
		-- The push runs where the `cd`s before it went.
		local dirs = jouet.dirs(call.commands)
		local dir: string | false | nil = nil
		for i, command in call.commands do
			if command.matched then
				dir = dirs[i]
				break
			end
		end
		if dir == false then
			return nil
		end
		local why = untested(dir or nil)
		if why then
			return jouet.warn(
				`Before you push, run the full test suite: {why}. Runs of one package, file or test do not count. If the full suite is not needed here, say why.`
			)
		end
		return nil
	end,
}

return rule

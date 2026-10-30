--!strict
-- The `push-untested` rule's script (push-untested.toml). Its matcher only
-- lets a push through, so the script asks whether the work tree is known to
-- pass the full test suite.

-- Why the work tree is not known to pass the full suite; nil when it is, or
-- when nothing was ever recorded here to judge by (the ledger is off, or
-- jouet's hooks have not seen a check in this work tree).
local function untested(): string?
	local any, err = jouet.ci.last_run()
	if any == nil and err == nil then
		return nil
	end
	local full = jouet.ci.last_run("test", "full")
	local tree = jouet.git.tree()
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
	check = function(_call: JouetCall): JouetDecision?
		local why = untested()
		if why then
			return jouet.warn(
				`Before you push, run the full test suite: {why}. Runs of one package, file or test do not count. If the full suite is not needed here, say why.`
			)
		end
		return nil
	end,
}

return rule

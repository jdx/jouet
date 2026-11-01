--!strict
-- The `affected-tests` rule's script (affected-tests.toml). Its matcher
-- only lets a whole-suite test run through; the script stays quiet unless
-- jouet can select tests here and the newest test run was also a whole
-- suite, recently, whose output the agent saw. It never says to skip the
-- full run at the end.

-- How recent the last full run must be for this one to count as iterating.
local RECENT = 30 * 60

-- Whether a run's command shape cut its output: then the agent did not see
-- the run, and running the suite again whole, as `piped-test-output` asks,
-- is not iterating.
local CUTS = { "| head", "| tail", "| grep", "| rg", "| sed" }
local function cut(shape: string?): boolean
	if not shape then
		return false
	end
	for _, pipe in CUTS do
		if string.find(shape, pipe, 1, true) then
			return true
		end
	end
	return false
end

local rule: JouetRule = {
	check = function(_call: JouetCall): JouetDecision?
		-- Without the probes (as in the rule's examples) both are unknown,
		-- and the rule fires on what the matcher found.
		local adapters, err = jouet.test.adapters()
		if not err and (not adapters or #adapters == 0) then
			return nil
		end
		local last, last_err = jouet.ci.last_run("test")
		if not last_err then
			if not last or last.scope ~= "full" then
				return nil
			end
			if last.ts and os.time() - last.ts > RECENT then
				return nil
			end
			if cut(last.shape) then
				return nil
			end
		end
		local which = if adapters and #adapters > 0 then ` ({table.concat(adapters, ", ")})` else ""
		return jouet.warn(
			`The whole test suite ran a few minutes ago. While you iterate, \`jouet test --affected\`{which} runs only the tests your changes affect, and \`--dry-run\` shows which and why. The full suite runs on CI after you push.`
		)
	end,
}

return rule

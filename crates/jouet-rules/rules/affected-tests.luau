--!strict
-- The `affected-tests` rule's script (affected-tests.toml). Its matcher
-- only lets a whole-suite test run through; the script stays quiet unless
-- jouet can select tests here and the newest test run was also a whole
-- suite, recently. It never says to skip the full run at the end.

-- How recent the last full run must be for this one to count as iterating.
local RECENT = 30 * 60

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
		end
		local which = if adapters and #adapters > 0 then ` ({table.concat(adapters, ", ")})` else ""
		return jouet.warn(
			`The whole test suite ran a few minutes ago. While you iterate, \`jouet test --affected\`{which} runs only the tests your changes affect, and \`--dry-run\` shows which and why. Keep the full run for the end: it still has to pass before you finish or push.`
		)
	end,
}

return rule

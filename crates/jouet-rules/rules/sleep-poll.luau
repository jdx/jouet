--!strict
--- name: sleep-poll
--- why: A sleep loop around a CI query spends a model turn on every check; `jouet wait ci` blocks until the checks settle.
--- mode: deny
--- scope: builtin
--- author: builtin
--- created: 2026-10-30

local function polls_ci(command: JouetCommand): boolean
	local args = jouet.gh(command)
	if not args then
		return false
	end
	if (args[1] == "pr" and args[2] == "checks") or (args[1] == "run" and args[2] == "view") then
		return true
	end
	if args[1] == "pr" and args[2] == "view" then
		for _, arg in args do
			if string.find(arg, "statusCheckRollup", 1, true) then
				return true
			end
		end
	end
	return false
end

local rule: JouetRule = {
	needs = { "wait", "ci" },
	check = function(call: JouetCall): JouetDecision?
		local sleeps, polls = false, false
		for _, command in call.commands do
			if command.program == "sleep" and #command.argv > 1 then
				sleeps = true
			end
			polls = polls or polls_ci(command)
		end
		if sleeps and polls then
			return jouet.deny(
				"Polling CI with sleep burns turns: `jouet wait ci` blocks until the checks settle and prints the failing logs (`jouet agent-help wait`)."
			)
		end
		return nil
	end,
	examples = {
		deny = {
			"sleep 60 && gh pr checks 123",
			"while true; do gh pr checks; sleep 30; done",
			"until gh run view 42 --exit-status; do sleep 20; done",
			"sleep 90; gh pr view 7 --json statusCheckRollup",
			"bash -c 'sleep 30; gh pr checks'",
		},
		allow = {
			"gh pr checks 123",
			"sleep 5 && curl localhost:8080",
			"echo 'sleep 30; gh pr checks'",
			"git commit -m 'sleep 60 then gh pr checks'",
			"gh pr checks --watch",
		},
	},
}

return rule

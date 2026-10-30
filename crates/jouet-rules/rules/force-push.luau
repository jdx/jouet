--!strict
--- name: force-push
--- why: A plain force push overwrites commits pushed since you last fetched; `--force-with-lease` refuses instead.
--- mode: deny
--- scope: builtin
--- author: builtin
--- created: 2026-10-30

local MESSAGE = "Use `git push --force-with-lease`: it refuses a push that would drop commits you have not seen."

local function forced(args: { string }): boolean
	local opts = jouet.options(args)
	for _, arg in opts do
		if string.sub(arg, 1, 18) == "--force-with-lease" then
			return false
		end
	end
	for _, arg in opts do
		-- `-o` takes a value, so `-of` is a push option, not a force.
		if arg == "--force" or jouet.short_flag(arg, "f", "o") then
			return true
		end
	end
	for _, arg in args do
		if #arg > 1 and string.sub(arg, 1, 1) == "+" then
			return true
		end
	end
	return false
end

local rule: JouetRule = {
	check = function(call: JouetCall): JouetDecision?
		for _, command in call.commands do
			local sub, args = jouet.git(command)
			if sub == "push" and forced(args) then
				return jouet.deny(MESSAGE)
			end
		end
		return nil
	end,
	examples = {
		fires = {
			"git push --force",
			"git push -f origin main",
			"git push -fu origin feat",
			"git push origin +main",
		},
		quiet = {
			"git push --force-with-lease",
			"git push --force-with-lease=main:abc123 --force-if-includes",
			"git push --force --force-with-lease",
			"git push origin main",
			"git push -o ci.skip origin main",
			"git commit -m 'git push --force'",
			"rm -f file && git push",
		},
	},
}

return rule

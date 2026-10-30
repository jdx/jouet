--!strict
--- name: no-verify
--- why: Skipping git hooks with `--no-verify` hides what they would have caught.
--- mode: deny
--- scope: builtin
--- author: builtin
--- created: 2026-10-30

local MESSAGE = "Do not skip git hooks with --no-verify: fix what the hook reports, or ask the user."

-- `git commit` options whose value is the next word, or the rest of the word.
local VALUE_SHORTS = "mFCct"
local VALUE_LONGS = {
	"--message",
	"--file",
	"--author",
	"--date",
	"--template",
	"--reuse-message",
	"--reedit-message",
	"--fixup",
	"--squash",
	"--trailer",
	"--cleanup",
}

-- `git commit -n` and `--no-verify`, skipping option values, so `-m -n` and
-- `-m --no-verify` are messages.
local function commit_skips_hooks(opts: { string }): boolean
	local i = 1
	while i <= #opts do
		local arg = opts[i]
		i += 1
		if jouet.contains(VALUE_LONGS, arg) then
			i += 1
		elseif arg == "--no-verify" or jouet.short_flag(arg, "n", VALUE_SHORTS) then
			return true
		elseif string.sub(arg, 1, 1) == "-" and string.sub(arg, 2, 2) ~= "-" then
			-- A cluster ending in an option that takes a value takes the next word.
			local last = string.sub(arg, -1)
			if #arg > 1 and string.find(VALUE_SHORTS, last, 1, true) then
				i += 1
			end
		end
	end
	return false
end

local rule: JouetRule = {
	check = function(call: JouetCall): JouetDecision?
		for _, command in call.commands do
			local sub, args = jouet.git(command)
			if sub then
				local opts = jouet.options(args)
				local skips = if sub == "commit" then commit_skips_hooks(opts) else jouet.contains(opts, "--no-verify")
				if skips then
					return jouet.deny(MESSAGE)
				end
			end
		end
		return nil
	end,
	examples = {
		fires = {
			"git commit --no-verify -m x",
			"git push --no-verify",
			"git commit -n -m x",
			"git commit -nm x",
			"git -c core.x=1 commit -am x --no-verify",
		},
		quiet = {
			"git commit -m 'drop --no-verify'",
			"git commit -m -n",
			"git commit -m '--no-verify'",
			"git commit --message --no-verify",
			"git commit -mn",
			"git push -n",
			"git log --no-verify-sig",
			"cargo publish --no-verify",
		},
	},
}

return rule

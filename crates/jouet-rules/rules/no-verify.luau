--!strict
-- Skipping git hooks hides what they would have caught.

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

-- `git commit -n` is `--no-verify`; option values are skipped, so `-m -n`
-- is a message.
local function commit_short_n(opts: { string }): boolean
	local i = 1
	while i <= #opts do
		local arg = opts[i]
		i += 1
		if jouet.contains(VALUE_LONGS, arg) then
			i += 1
		elseif jouet.short_flag(arg, "n", VALUE_SHORTS) then
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
	description = "Deny `--no-verify` and `git commit -n`",
	check = function(call: JouetCall): JouetDecision?
		for _, command in call.commands do
			local sub, args = jouet.git(command)
			if sub then
				local opts = jouet.options(args)
				if jouet.contains(opts, "--no-verify") or (sub == "commit" and commit_short_n(opts)) then
					return jouet.deny(MESSAGE)
				end
			end
		end
		return nil
	end,
	examples = {
		deny = {
			"git commit --no-verify -m x",
			"git push --no-verify",
			"git commit -n -m x",
			"git commit -nm x",
			"git -c core.x=1 commit -am x --no-verify",
		},
		allow = {
			"git commit -m 'drop --no-verify'",
			"git commit -m -n",
			"git commit -mn",
			"git push -n",
			"git log --no-verify-sig",
			"cargo publish --no-verify",
		},
	},
}

return rule

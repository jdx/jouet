--!strict
-- `git add -A` and `git add .` stage everything in the worktree: caches,
-- scratch files, and work another session left there.

local MESSAGE =
	"Stage explicit paths (`git add path/to/file ...`): `git add -A` and `git add .` sweep in caches, scratch files and other sessions' work."

local EVERYTHING = { ".", "./", ":/", ":/." }

local rule: JouetRule = {
	description = "Deny `git add -A`, `--all` and `.`; stage explicit paths",
	check = function(call: JouetCall): JouetDecision?
		for _, command in call.commands do
			local sub, args = jouet.git(command)
			if sub == "add" then
				for _, arg in jouet.options(args) do
					if arg == "--all" or jouet.short_flag(arg, "A") then
						return jouet.deny(MESSAGE)
					end
				end
				for _, arg in args do
					if jouet.contains(EVERYTHING, arg) then
						return jouet.deny(MESSAGE)
					end
				end
			end
		end
		return nil
	end,
	examples = {
		deny = {
			"git add -A",
			"git add --all",
			"git add .",
			"git -C repo add -A && git commit -m x",
			"git add -fA",
			"git add -- .",
			"rtk git add .",
		},
		allow = {
			"git add src/main.rs",
			"git add ./src/main.rs",
			"git commit -m 'git add -A'",
			"git commit -m \"$(cat <<'EOF'\nnever git add -A\nEOF\n)\"",
			"echo git add .",
			"git add -u",
			"git add -p src",
		},
	},
}

return rule

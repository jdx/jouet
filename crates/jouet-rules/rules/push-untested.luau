--!strict
--- name: push-untested
--- why: Pushing a tree that no full test run has passed at hands CI and reviewers untested code; a partial run does not count.
--- mode: warn
--- scope: builtin
--- author: builtin
--- created: 2026-10-30

-- `git push` options that push nothing new: a dry run, deletions, help.
local PUSHES_NOTHING = { "-n", "--dry-run", "-d", "--delete", "-h", "--help" }

local function pushes(command: JouetCommand): boolean
	local sub, args = jouet.git(command)
	if sub == "push" then
		for _, arg in jouet.options(args) do
			if jouet.contains(PUSHES_NOTHING, arg) then
				return false
			end
			-- `git push origin :branch` deletes the branch.
			if string.sub(arg, 1, 1) == ":" then
				return false
			end
		end
		return true
	end
	local gh = jouet.gh(command)
	return gh ~= nil
		and gh[1] == "pr"
		and gh[2] == "create"
		and not jouet.contains(gh, "--dry-run")
		and not jouet.contains(gh, "--help")
end

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
	check = function(call: JouetCall): JouetDecision?
		for _, command in call.commands do
			if pushes(command) then
				local why = untested()
				if why then
					return jouet.warn(
						`Before you push, run the full test suite: {why}. Runs of one package, file or test do not count. If the full suite is not needed here, say why.`
					)
				end
				return nil
			end
		end
		return nil
	end,
	examples = {
		warn = {
			"git push",
			"git push -u origin feat/verify",
			"git -C ../repo push --force-with-lease",
			"cargo test -p x && git push",
			"gh pr create --fill",
		},
		allow = {
			"git push --dry-run",
			"git push origin --delete old-branch",
			"git push origin :old-branch",
			"git pull",
			"git status",
			"echo 'git push'",
			"gh pr view",
			"gh pr create --help",
		},
	},
}

return rule

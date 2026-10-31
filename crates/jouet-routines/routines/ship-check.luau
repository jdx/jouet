--- name: ship-check
--- about: Before saying done: run the project's tests, then check the PR, and report both
--- help: Fails when the tests fail or the PR needs attention; without a PR for this
--- help: branch it reports the tests alone.
--- capabilities: github.read, mise.run
--- usage: arg "[pr]" help="The PR: a number, URL, OWNER/REPO#N or branch. Defaults to the current branch's PR"
--- usage: flag "--repo <OWNER/NAME>" help="The repository for a bare number or branch. Defaults to the git remote"

return {
	run = function(args)
		local test = mise.test()
		local command = table.concat(test.command, " ")
		if test.ok then
			print(`tests passed: {command}`)
		else
			print(`tests failed (exit {test.exit_code}): {command}`)
			print(`  log: {test.log}`)
		end
		local ok, found = pcall(github.prs, { prs = if args.pr then { args.pr } else nil, repo = args.repo, settle = 0 })
		local pr = ok and found.prs[1] or nil
		local attention = joue.array({})
		if pr then
			local checks = pr.checks or {}
			local total = (checks.pass or 0) + (checks.fail or 0) + (checks.pending or 0) + (checks.skipped or 0)
			print(
				`{pr.repo}#{pr.number} {pr.state}: ci {checks.pass or 0}/{total}, threads {pr.unresolved_threads or 0}, review {pr.review or "-"}`
			)
			for _, a in pr.attention or {} do
				table.insert(attention, a.code)
				print(`  {a.code}: {a.message}`)
			end
		else
			print(if ok then "no PR for this branch" else `no PR status: {found}`)
		end
		local passed = test.ok and #attention == 0
		local code = if passed then 0 elseif not test.ok then test.exit_code else joue.exit.failed
		return joue.object({
			ok = passed,
			tests = joue.object(
				{ ok = test.ok, exit_code = test.exit_code, command = command, log = test.log },
				{ "ok", "exit_code", "command", "log" }
			),
			pr = pr,
			attention = attention,
		}, { "ok", "tests", "pr", "attention" }), code
	end,

	examples = {
		{
			name = "passing tests and a PR with an unresolved thread is not done",
			args = { "jdx/jouet#15" },
			fixture = "fixtures/ship-check.json",
			exit = 1,
			output = { "tests passed: cargo test", "jdx/jouet#15 open", "unresolved_threads:" },
			json = { ok = false, tests = { ok = true }, attention = { "unresolved_threads" } },
		},
	},
}

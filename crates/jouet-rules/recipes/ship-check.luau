--- why: before saying done: run the repository's test task, then pr status, and report both
--- author: builtin

-- `jouet run ship-check [PR]`. Fails when the tests fail or the PR needs
-- attention; without a PR for this branch it reports the tests alone.

return {
	usage = "[PR]: the PR (default: this branch's)",
	run = function(args)
		local pr = args.pr or args[1]
		local test = jouet.cmd.test()
		local lines = {}
		if test.ok then
			table.insert(lines, `tests passed: {test.command}`)
		else
			table.insert(lines, `tests failed (exit {test.exit_code}): {test.command}`)
			if test.json.log then
				table.insert(lines, `  log: {test.json.log}`)
			end
		end
		local status = jouet.cmd.pr_status({ prs = pr })
		local found = status.json.prs and status.json.prs[1]
		local attention = {}
		if found then
			local checks = found.checks or {}
			local total = (checks.pass or 0) + (checks.fail or 0) + (checks.pending or 0) + (checks.skipped or 0)
			table.insert(
				lines,
				`{found.repo}#{found.number} {found.state}: ci {checks.pass or 0}/{total}, threads {found.unresolved_threads or 0}, review {found.review or "-"}`
			)
			for _, a in found.attention or {} do
				table.insert(attention, a.code)
				table.insert(lines, `  {a.code}: {a.message}`)
			end
		elseif not status.dry_run then
			table.insert(lines, status.error or "no PR for this branch")
		end
		local ok = test.ok and #attention == 0
		return {
			ok = ok,
			exit_code = if ok then 0 elseif not test.ok then test.exit_code else 1,
			message = table.concat(lines, "\n"),
			tests = { ok = test.ok, exit_code = test.exit_code, command = test.command },
			pr = found,
			attention = attention,
		}
	end,
}

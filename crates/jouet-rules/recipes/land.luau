--- why: land this branch's PR: pr next, wait for CI if it is still running, then pr land
--- author: builtin

-- `jouet run land [PR] [yes=true]`. Without `yes=true` the last step is
-- `pr land --dry-run`, which shows the plan and merges nothing.

local function first(result)
	return result.json.prs and result.json.prs[1]
end

local function stop(result, message)
	return { ok = false, exit_code = result.exit_code, message = message }
end

return {
	usage = "[PR] [yes=true]: the PR (default: this branch's); yes=true merges",
	run = function(args)
		local pr = args.pr or args[1]
		local next = jouet.cmd.pr_next({ prs = pr })
		local entry = first(next)
		if not next.dry_run and not entry then
			local missing = next.json.not_found and next.json.not_found[1]
			return stop(next, next.error or (if missing then `no PR for {missing}` else "no PR to land"))
		end
		if next.dry_run or entry.action == "checks_pending" then
			local ci = jouet.cmd.wait_ci({ pr = pr })
			if not ci.ok then
				return stop(ci, ci.error or "CI did not pass")
			end
			next = jouet.cmd.pr_next({ prs = pr })
			entry = first(next)
		end
		if not next.dry_run and (not entry or entry.action ~= "ready_to_merge") then
			local why = entry and `{entry.action}: {entry.reason}` or (next.error or "not ready")
			return stop(next, why)
		end
		local target = pr or (entry and entry.number and tostring(entry.number))
		local land = if args.yes
			then jouet.cmd.pr_land({ pr = target, yes = true })
			else jouet.cmd.pr_land({ pr = target, dry_run = true })
		if not land.ok then
			return stop(land, land.error or "pr land refused")
		end
		local result = land.json.result or "dry_run"
		local message = if result == "merged"
			then `merged {land.json.repo}#{land.json.pr}`
			elseif args.yes then `pr land: {result}`
			else "ready to land; run again with yes=true to merge"
		return { ok = true, message = message, land = land.json }
	end,
}

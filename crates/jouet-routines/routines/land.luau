--- name: land
--- about: Land a PR: ask pr-next, wait for CI if it is still running, then pr land
--- help: Without --yes the land is only previewed (`jouet pr land --dry-run`), and
--- help: joue exits 4 saying what it would do. Refuses unless pr-next says
--- help: ready_to_merge, and `jouet pr land` still refuses whatever it refuses by
--- help: hand, honouring branch protection.
--- capabilities: github.read, github.write
--- usage: arg "[pr]" help="The PR: a number, URL, OWNER/REPO#N or branch. Defaults to the current branch's PR"
--- usage: flag "--repo <OWNER/NAME>" help="The repository for a bare number or branch. Defaults to the git remote"
--- usage: flag "--method <METHOD>" help="How to merge: squash, merge or rebase (default squash)"

local function next_of(args)
	local words = {}
	if args.pr then
		table.insert(words, args.pr)
	end
	if args.repo then
		table.insert(words, "--repo")
		table.insert(words, args.repo)
	end
	local report = joue.call("pr-next", words)
	local entry = report.prs[1]
	if not entry then
		local missing = report.not_found and report.not_found[1]
		joue.fail(joue.exit.not_found, if missing then `no PR for {missing}` else "no PR to land")
	end
	return entry
end

return {
	run = function(args)
		local entry = next_of(args)
		local name = `{entry.repo}#{entry.number}`
		if entry.action == "checks_pending" then
			print(`{name}: {entry.reason}; waiting for CI`)
			local ci = wait.ci({ pr = name })
			if not ci.ok then
				print(ci.error or `CI did not pass (exit {ci.exit_code})`)
				return { pr = name, result = "ci_failed", ci = ci.json }, ci.exit_code
			end
			entry = next_of({ pr = name })
		end
		if entry.action ~= "ready_to_merge" then
			print(`{name} is not ready to land: {entry.action}: {entry.reason}`)
			if entry.command then
				print(`next: {entry.command}`)
			end
			return { pr = name, result = "not_ready", next = entry }, joue.exit.precondition
		end
		local land = jouet.pr_land({ pr = name, method = args.method })
		if not land.ok then
			print(land.error or `pr land refused (exit {land.exit_code})`)
			return { pr = name, result = land.json.result or "refused", land = land.json }, land.exit_code
		end
		if land.json.result == "merged" then
			print(`merged {name}{if land.json.merge_commit then ` as {land.json.merge_commit}` else ""}`)
		else
			print(`{name} is ready to land: {entry.reason}`)
			for _, step in land.json.plan or {} do
				print(`  {step}`)
			end
		end
		return { pr = name, result = land.json.result, land = land.json }
	end,

	examples = {
		{
			name = "without --yes the land is previewed, and nothing merges",
			args = { "jdx/jouet#19" },
			fixture = "fixtures/land.json",
			exit = 4,
			output = { "jdx/jouet#19 is ready to land", "merge with squash" },
			planned = { "jouet pr land jdx/jouet#19" },
			requests = { "jouet pr land jdx/jouet#19 --dry-run" },
			json = { result = "dry_run", planned = { "jouet pr land jdx/jouet#19" } },
		},
		{
			name = "with --yes it lands",
			args = { "jdx/jouet#19" },
			yes = true,
			fixture = "fixtures/land.json",
			output = "merged jdx/jouet#19 as 0123abc",
			planned = {},
			requests = { "jouet pr land jdx/jouet#19 --yes" },
		},
		{
			name = "running checks are waited for before landing",
			args = { "jdx/jouet#19" },
			yes = true,
			fixture = "fixtures/land-pending.json",
			output = { "waiting for CI", "merged jdx/jouet#19" },
			requests = { "jouet wait ci jdx/jouet#19", "jouet pr land jdx/jouet#19 --yes" },
		},
		{
			name = "a PR that is not ready is refused before anything is posted",
			args = { "jdx/jouet#15" },
			fixture = "fixtures/land.json",
			exit = 4,
			output = { "is not ready to land: unresolved_threads", "next: jouet pr threads jdx/jouet#15" },
			planned = {},
		},
	},
}

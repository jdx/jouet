--- name: pr-next
--- about: Say the one thing each PR needs next, and the command for it
--- help: Works through a fixed order and stops at the first state that applies:
--- help: merged, closed, draft, conflict, mergeable_unknown, behind_base,
--- help: checks_failing, checks_pending, changes_requested, unresolved_threads,
--- help: awaiting_approval, blocked, auto_merge, ready_to_merge. The states come
--- help: from the same data and attention codes as `jouet pr status`. Exits 0
--- help: when nothing is left to do, 10 when the agent must act and 20 when
--- help: waiting on someone or something else; with several PRs, 10 wins.
--- capabilities: github.read
--- usage: arg "[prs]..." help="PRs as numbers, URLs, OWNER/REPO#N or branch names. Defaults to the current branch's PR"
--- usage: flag "--mine" help="Add the open PRs authored by the token's user"
--- usage: flag "--search <QUERY>" help="Add the PRs matching a GitHub search query"
--- usage: flag "--repo <OWNER/NAME>" var=#true help="The repository for bare numbers, branches, --mine and --search. Repeatable. Defaults to the git remote"
--- usage: flag "--settle <DUR>" help="How long to wait for GitHub to compute an unknown mergeable state (default 20s; 0 disables)"

-- `jouet pr next`, as a routine: the same selection and status as `pr
-- status` (github.prs), then the decision, the text and the exit code here.

type Pr = {
	repo: string,
	-- `gitlab` or `forgejo`, with the host, for a PR not on GitHub.
	forge: string?,
	host: string?,
	number: number,
	title: string,
	url: string,
	state: string,
	base: string,
	merge_state: string,
	checks: { pass: number?, fail: number?, pending: number?, skipped: number? }?,
	review: string?,
	auto_merge: boolean?,
	attention: { { code: string, message: string } }?,
}

local KIND = {
	merged = "done",
	closed = "done",
	ready_to_merge = "done",
	draft = "act",
	conflict = "act",
	behind_base = "act",
	checks_failing = "act",
	changes_requested = "act",
	unresolved_threads = "act",
	mergeable_unknown = "wait",
	checks_pending = "wait",
	awaiting_approval = "wait",
	blocked = "wait",
	auto_merge = "wait",
}

-- GitHub's merge states that let a PR merge: `unstable` is a failing check
-- branch protection does not require, `has_hooks` pre-receive hooks.
local MERGEABLE = { clean = true, unstable = true, has_hooks = true }

local function attention(pr: Pr, code: string): string?
	local list: { { code: string, message: string } } = pr.attention or {}
	for _, a in list do
		if a.code == code then
			return a.message
		end
	end
	return nil
end

-- The forge's name in messages.
local FORGE = { gitlab = "GitLab", forgejo = "Forgejo" }

-- The first state that applies: (action, reason, command).
local function decide(pr: Pr): (string, string, string?)
	local qualified = if pr.host then `{pr.host}/{pr.repo}` else pr.repo
	local name = `{qualified}#{pr.number}`
	local forge = FORGE[pr.forge or ""] or "GitHub"
	local update = `git fetch origin {pr.base} && git merge origin/{pr.base}`
	if pr.state == "merged" then
		return "merged", "Merged", nil
	elseif pr.state == "closed" then
		return "closed", "Closed without merging; reopen it to continue", nil
	end
	local reason = attention(pr, "draft")
	if reason then
		if pr.forge == "gitlab" then
			return "draft", reason, `glab mr update {pr.number} --ready -R {qualified}`
		elseif pr.forge == "forgejo" then
			-- A Forgejo draft is a `WIP:` title, which no command clears.
			return "draft", reason, nil
		end
		return "draft", reason, `gh pr ready {pr.number} --repo {pr.repo}`
	end
	reason = attention(pr, "conflict")
	if reason then
		return "conflict", reason, update
	end
	reason = attention(pr, "mergeable_unknown")
	if reason then
		return "mergeable_unknown", reason, `jouet wait pr {name} --until mergeable`
	end
	reason = attention(pr, "behind_base")
	if reason then
		return "behind_base", reason, update
	end
	reason = attention(pr, "checks_failing")
	if reason then
		return "checks_failing", reason, `jouet wait ci {name}`
	end
	local checks = pr.checks or {}
	local pending = checks.pending or 0
	local total = (checks.pass or 0) + (checks.fail or 0) + pending + (checks.skipped or 0)
	-- No checks yet, but not for long enough that `pr status` calls them
	-- missing: they are still being registered.
	local registering = total == 0 and not attention(pr, "checks_missing")
	if pending > 0 then
		return "checks_pending", `{pending} of {total} checks pending`, `jouet wait ci {name}`
	elseif registering then
		return "checks_pending", "Waiting for checks to register on the new head", `jouet wait ci {name}`
	end
	reason = attention(pr, "changes_requested")
	if reason then
		return "changes_requested", reason, `jouet pr threads {name} --include-reviews`
	end
	reason = attention(pr, "unresolved_threads")
	if reason then
		return "unresolved_threads", reason, `jouet pr threads {name}`
	end
	if pr.review == "review_required" then
		return "awaiting_approval", "Waiting for an approving review", `jouet wait pr {name} --until approved`
	end
	if pr.merge_state == "unknown" then
		return "mergeable_unknown",
			`{forge} has not finished computing the merge state; run again shortly`,
			`jouet wait pr {name} --until mergeable`
	end
	if not MERGEABLE[pr.merge_state] then
		local why = if pr.forge then "" else " by branch protection or a ruleset"
		return "blocked",
			`{forge} reports the merge as {pr.merge_state}{why}`,
			`jouet wait pr {name} --until any-change`
	end
	if pr.auto_merge then
		return "auto_merge", `Auto-merge is enabled; {forge} will merge it`, `jouet wait pr {name} --until merged`
	end
	local ran = if total == 0 then "No checks ran" else `Checks pass ({checks.pass or 0}/{total})`
	local review = if pr.review == "approved" then "approved" else "no review required"
	local land = `jouet pr land {name}`
	if pr.forge == "gitlab" then
		land = `glab mr merge {pr.number} -R {qualified}`
	elseif pr.forge == "forgejo" then
		land = `tea pulls merge {pr.number} --repo {pr.repo}`
	end
	return "ready_to_merge", `{ran}, {review}, no unresolved threads`, land
end

local function pad(text: string, width: number): string
	return text .. string.rep(" ", width - #text)
end

local function render(report): string
	local prs = report.prs
	local one_repo = true
	for i = 2, #prs do
		if prs[i].repo ~= prs[i - 1].repo then
			one_repo = false
		end
	end
	local function name(e): string
		return if one_repo then `#{e.number}` else `{e.repo}#{e.number}`
	end
	local width = 0
	for _, e in prs do
		width = math.max(width, #name(e))
	end
	local lines = {}
	for _, e in prs do
		table.insert(lines, `{pad(name(e), width)} {pad(e.kind, 4)} {e.action}: {e.reason}`)
		if e.command then
			table.insert(lines, `{pad("", width)} next: {e.command}`)
		end
	end
	if report.not_found then
		table.insert(lines, `not found: {table.concat(report.not_found, ", ")}`)
	end
	local unlisted = 0
	for _, i in report.incomplete or {} do
		table.insert(
			lines,
			`search \`{i.query}\` matched {i.matched} PRs but GitHub listed only {i.listed} (it lists at most 1000)`
		)
		unlisted += i.matched - i.listed
	end
	local summary = `{#prs} {if #prs == 1 then "PR" else "PRs"}`
	local parts = {}
	for _, group in { { "act", "to act on" }, { "wait", "waiting" }, { "done", "done" } } do
		local names = {}
		for _, e in prs do
			if e.kind == group[1] then
				table.insert(names, name(e))
			end
		end
		if #names > 0 then
			table.insert(parts, `{#names} {group[2]} ({table.concat(names, ", ")})`)
		end
	end
	if #parts > 0 then
		summary ..= ": " .. table.concat(parts, ", ")
	end
	if report.not_found then
		summary ..= `; {#report.not_found} not found`
	end
	if unlisted > 0 then
		summary ..= `; {unlisted} more matched the search but were not listed`
	end
	table.insert(lines, summary)
	return table.concat(lines, "\n")
end

return {
	run = function(args)
		local settle = nil
		if args.settle then
			local seconds, err = joue.duration(args.settle)
			if not seconds then
				joue.fail(joue.exit.usage, `--settle: {err}`)
			end
			settle = seconds
		end
		local found = github.prs({
			prs = args.prs,
			mine = args.mine,
			search = args.search,
			repo = args.repo,
			settle = settle,
		})
		local entries = joue.array({})
		local any = { act = false, wait = false }
		for _, pr in found.prs do
			local action, reason, command = decide(pr)
			local kind = KIND[action]
			any[kind] = true
			table.insert(
				entries,
				joue.object({
					repo = pr.repo,
					host = pr.host,
					number = pr.number,
					title = pr.title,
					url = pr.url,
					action = action,
					kind = kind,
					reason = reason,
					command = command,
				}, { "repo", "host", "number", "title", "url", "action", "kind", "reason", "command" })
			)
		end
		local report = joue.object({
			prs = entries,
			not_found = found.not_found,
			incomplete = found.incomplete,
		}, { "prs", "not_found", "incomplete" })
		if not joue.json then
			print(render(report))
		end
		local code = if found.not_found
			then joue.exit.not_found
			elseif any.act then joue.exit.act
			elseif any.wait then joue.exit.waiting
			else joue.exit.ok
		return report, code
	end,

	examples = {
		{
			name = "a merged PR is done and needs nothing",
			args = { "15187", "--repo", "jdx/mise" },
			fixture = "fixtures/pr-next.json",
			output = { "#15187 done merged: Merged", "1 PR: 1 done (#15187)" },
			json = { prs = { { repo = "jdx/mise", number = 15187, action = "merged", kind = "done" } } },
			requests = { "POST /graphql" },
		},
		{
			name = "running checks are waited for, with the wait command",
			args = { "jdx/jouet#19" },
			fixture = "fixtures/pr-next.json",
			exit = 20,
			output = { "#19 wait checks_pending: 5 of 15 checks pending", "next: jouet wait ci jdx/jouet#19" },
			json = { prs = { { action = "checks_pending", command = "jouet wait ci jdx/jouet#19" } } },
		},
		{
			name = "a draft is the agent's to mark ready, and act wins over done",
			args = { "15172", "15187", "--repo", "jdx/mise" },
			fixture = "fixtures/pr-next.json",
			exit = 10,
			output = {
				"#15172 act  draft: The PR is a draft",
				"next: gh pr ready 15172 --repo jdx/mise",
				"2 PRs: 1 to act on (#15172), 1 done (#15187)",
			},
		},
		{
			name = "a PR that is not there is not found",
			args = { "404", "--repo", "jdx/mise" },
			fixture = "fixtures/pr-next.json",
			exit = 3,
			output = "not found: jdx/mise#404",
			json = { prs = {}, not_found = { "jdx/mise#404" } },
		},
		{
			name = "PRs in more than one repository are named with it",
			args = { "jdx/jouet#19", "jdx/mise#15187" },
			fixture = "fixtures/pr-next.json",
			exit = 20,
			output = {
				"jdx/jouet#19   wait checks_pending: 5 of 15 checks pending",
				"jdx/mise#15187 done merged: Merged",
				"2 PRs: 1 waiting (jdx/jouet#19), 1 done (jdx/mise#15187)",
			},
		},
		{
			name = "a search GitHub cut short says how many it left out",
			args = { "--search", "review-requested:@me", "--repo", "jdx/mise" },
			fixture = "fixtures/pr-next.json",
			output = {
				"search `is:pr review-requested:@me repo:jdx/mise` matched 1200 PRs but GitHub listed only 1 (it lists at most 1000)",
				"1 PR: 1 done (#15187); 1199 more matched the search but were not listed",
			},
			json = { incomplete = { { matched = 1200, listed = 1 } } },
		},
		{
			name = "without a PR or a remote there is nothing to name",
			args = { "7" },
			exit = 2,
			output = "jouet: no --repo given and no GitHub remote here, so `7` names no repository",
		},
		{
			name = "a bad --settle is a usage error",
			args = { "7", "--repo", "jdx/demo", "--settle", "soon" },
			exit = 2,
			output = "jouet: --settle: `soon` is not a duration",
		},
	},
}

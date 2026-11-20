--- name: sweep
--- about: List a PR's unresolved review threads as a compact work list, posting nothing
--- help: One line per thread: its ID (the first comment's, as `jouet pr threads`
--- help: takes it), where it is, who started it and the first line of what they
--- help: said. Reply and resolve with `jouet pr threads reply|resolve|apply` once
--- help: the work is done.
--- capabilities: github.read
--- usage: arg "[pr]" help="The PR: a number, URL, OWNER/REPO#N or branch. Defaults to the current branch's PR"
--- usage: flag "--repo <OWNER/NAME>" help="The repository for a bare number or branch. Defaults to the git remote"

local function first_line(text: string?): string
	local line = string.match(text or "", "^[^\n]*") or ""
	if #line > 100 then
		line = string.sub(line, 1, 97) .. "..."
	end
	return line
end

return {
	run = function(args)
		local found = github.prs({ prs = if args.pr then { args.pr } else nil, repo = args.repo, settle = 0 })
		local pr = found.prs[1]
		if not pr then
			local missing = found.not_found and found.not_found[1] or "this branch"
			joue.fail(joue.exit.not_found, `no PR for {missing}`)
		end
		-- Read from whichever forge the PR is on, as `jouet pr threads` reads it.
		local found_threads = github.threads(pr)
		local items = joue.array({})
		local lines = {}
		for _, thread in found_threads.threads do
			local comment = thread.comments[1]
			if not thread.resolved and comment then
				local place = if thread.path then `{thread.path}:{thread.line or "?"}` else "conversation"
				local author = if thread.author ~= "" then thread.author else nil
				local item = joue.object({
					id = thread.id,
					place = place,
					author = author,
					outdated = thread.outdated == true,
					says = first_line(comment.body),
				}, { "id", "place", "author", "outdated", "says" })
				table.insert(items, item)
				table.insert(lines, `{item.id}  {place}  {author or "?"}: {item.says}`)
			end
		end
		-- A PR on another forge keeps its host, so the reference parses back to it.
		local ref = if pr.host then `{pr.host}/{pr.repo}#{pr.number}` else `{pr.repo}#{pr.number}`
		local header = `{#items} unresolved thread{if #items == 1 then "" else "s"} on {ref}`
		print(header)
		for _, line in lines do
			print(line)
		end
		return joue.object({ pr = ref, threads = items }, { "pr", "threads" })
	end,

	examples = {
		{
			name = "unresolved threads become one line each; resolved ones are left out",
			args = { "jdx/jouet#15" },
			fixture = "fixtures/sweep.json",
			output = {
				"2 unresolved threads on jdx/jouet#15",
				"2468  src/main.rs:12  octocat: Handle the empty case here",
				"2470  conversation  hubot: Long",
			},
			json = {
				pr = "jdx/jouet#15",
				threads = { { id = 2468, outdated = false }, { id = 2470, outdated = true } },
			},
			requests = { "POST /graphql", "reviewThreads" },
		},
		{
			-- The fixture also has a GitHub mirror at the same path with a PR
			-- of the same number; its thread must not be listed under the MR.
			name = "a GitLab MR's discussions are read from GitLab, not a GitHub repository at the same path",
			args = { "gitlab.com/group/sub/project!4266" },
			fixture = "fixtures/sweep-gitlab.json",
			output = {
				"2 unresolved threads on gitlab.com/group/sub/project#4266",
				"1126  internal/oauth/flow.go:42  reviewer: Should this reject an empty client ID too?",
				"1150  internal/oauth/client.go:?  maintainer: Rename this before it ships",
			},
			json = {
				pr = "gitlab.com/group/sub/project#4266",
				threads = { { id = 1126, outdated = false }, { id = 1150, outdated = true } },
			},
			requests = { "GET /projects/group%2Fsub%2Fproject/merge_requests/4266/discussions" },
		},
	},
}

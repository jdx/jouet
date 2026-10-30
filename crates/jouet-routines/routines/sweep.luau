--- name: sweep
--- about: List a PR's unresolved review threads as a compact work list, posting nothing
--- help: One line per thread: its ID (the first comment's, as `jouet pr threads`
--- help: takes it), where it is, who started it and the first line of what they
--- help: said. Reply and resolve with `jouet pr threads reply|resolve|apply` once
--- help: the work is done.
--- capabilities: github.read
--- usage: arg "[pr]" help="The PR: a number, URL, OWNER/REPO#N or branch. Defaults to the current branch's PR"
--- usage: flag "--repo <OWNER/NAME>" help="The repository for a bare number or branch. Defaults to the git remote"

local QUERY = [[
query($owner: String!, $name: String!, $number: Int!, $after: String) {
  repository(owner: $owner, name: $name) {
    pullRequest(number: $number) {
      reviewThreads(first: 100, after: $after) {
        pageInfo { hasNextPage endCursor }
        nodes {
          isResolved isOutdated path line
          comments(first: 1) { nodes { databaseId author { login } body } }
        }
      }
    }
  }
}]]

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
		local owner, name = string.match(pr.repo, "^([^/]+)/(.+)$")
		local items = joue.array({})
		local lines = {}
		local after = nil
		repeat
			local data = github.graphql(QUERY, { owner = owner, name = name, number = pr.number, after = after })
			local threads = data.repository.pullRequest.reviewThreads
			for _, thread in threads.nodes do
				local comment = thread.comments.nodes[1]
				if not thread.isResolved and comment then
					local place = if thread.path then `{thread.path}:{thread.line or "?"}` else "conversation"
					local item = joue.object({
						id = comment.databaseId,
						place = place,
						author = comment.author and comment.author.login,
						outdated = thread.isOutdated,
						says = first_line(comment.body),
					}, { "id", "place", "author", "outdated", "says" })
					table.insert(items, item)
					table.insert(lines, `{item.id}  {place}  {item.author or "?"}: {item.says}`)
				end
			end
			after = if threads.pageInfo.hasNextPage then threads.pageInfo.endCursor else nil
		until after == nil
		local header = `{#items} unresolved thread{if #items == 1 then "" else "s"} on {pr.repo}#{pr.number}`
		print(header)
		for _, line in lines do
			print(line)
		end
		return joue.object({ pr = `{pr.repo}#{pr.number}`, threads = items }, { "pr", "threads" })
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
	},
}

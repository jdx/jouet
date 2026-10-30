--!strict
--- name: review-threads
--- why: Raw `reviewThreads` GraphQL is long and easy to get wrong; `jouet pr threads` lists, replies to and resolves threads.
--- mode: deny
--- scope: builtin
--- author: builtin
--- created: 2026-10-30

local function graphql(command: JouetCommand): boolean
	local args = jouet.gh(command)
	if args then
		return args[1] == "api" and args[2] == "graphql"
	end
	if command.program == "curl" then
		for _, arg in command.argv do
			if string.find(arg, "api.github.com/graphql", 1, true) then
				return true
			end
		end
	end
	return false
end

local function mentions_threads(command: JouetCommand): boolean
	for _, text in command.argv do
		if string.find(text, "reviewThreads", 1, true) then
			return true
		end
	end
	for _, text in command.heredocs do
		if string.find(text, "reviewThreads", 1, true) then
			return true
		end
	end
	return false
end

local rule: JouetRule = {
	needs = { "pr", "threads" },
	check = function(call: JouetCall): JouetDecision?
		for _, command in call.commands do
			if graphql(command) and mentions_threads(command) then
				return jouet.deny(
					"Raw reviewThreads GraphQL is long and easy to get wrong: `jouet pr threads` lists, replies to and resolves review threads (`jouet agent-help pr`)."
				)
			end
		end
		return nil
	end,
	examples = {
		fires = {
			"gh api graphql -f query='{ repository(owner:\"a\",name:\"b\") { pullRequest(number:1) { reviewThreads(first:100) { nodes { id } } } } }'",
			"gh api graphql -F query=@- <<'EOF'\n{ reviewThreads(first: 50) { nodes { isResolved } } }\nEOF",
		},
		quiet = {
			"rg reviewThreads src/",
			"gh api graphql -f query='{ viewer { login } }'",
			"echo 'gh api graphql reviewThreads'",
		},
	},
}

return rule

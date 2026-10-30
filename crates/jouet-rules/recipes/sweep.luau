--- why: list a PR's unresolved review threads as a compact work list, posting nothing
--- author: builtin

-- `jouet run sweep [PR]`. One line per thread: its ID, where it is, who
-- started it and the first line of what they said. Reply and resolve with
-- `jouet pr threads reply|resolve|apply` once the work is done.

local function first_line(text: string?): string
	local line = string.match(text or "", "^[^\n]*") or ""
	if #line > 100 then
		line = string.sub(line, 1, 97) .. "..."
	end
	return line
end

return {
	usage = "[PR]: the PR (default: this branch's)",
	run = function(args)
		local threads = jouet.cmd.pr_threads({ pr = args.pr or args[1] })
		if not threads.ok then
			return { ok = false, exit_code = threads.exit_code, message = threads.error or "pr threads failed" }
		end
		local items = {}
		local lines = {}
		for _, thread in threads.json.threads or {} do
			local comment = thread.comments and thread.comments[1]
			local place = thread.path and `{thread.path}:{thread.line or "?"}` or "conversation"
			local item = {
				id = thread.id,
				place = place,
				author = thread.author,
				outdated = thread.outdated,
				says = first_line(comment and comment.body),
			}
			table.insert(items, item)
			table.insert(lines, `{item.id}  {place}  {item.author or "?"}: {item.says}`)
		end
		local pr = threads.json.pr and `{threads.json.repo}#{threads.json.pr}` or "the PR"
		local header = `{#items} unresolved thread{if #items == 1 then "" else "s"} on {pr}`
		if #items > 0 then
			header ..= "\n" .. table.concat(lines, "\n")
		end
		return { ok = true, message = header, threads = items }
	end,
}

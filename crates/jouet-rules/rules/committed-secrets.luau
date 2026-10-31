--!strict
-- The `committed-secrets` rule's script (committed-secrets.toml). Its matcher
-- lets a commit or a push through, so the script asks gitleaks' rules what
-- the lines it would add hold.

-- Findings named in the message; the rest are counted.
local LISTED = 5

local LIST_OUTGOING = "`jouet secrets scan --diff @{upstream}`"

-- The value-taking short options of `git commit`.
local COMMIT_VALUES = "mFCctS"

-- `git commit`'s long options that take the next word as their value.
local COMMIT_LONG = {
	"--message", "--file", "--reuse-message", "--reedit-message", "--fixup", "--squash",
	"--author", "--date", "--template", "--cleanup", "--trailer", "--pathspec-from-file",
}

-- A command's words that are not options, those after `--` included.
-- `values` names the short options that take a value and `long` the long
-- ones, whose next word is skipped.
local function operands(args: { string }, values: string, long: { string }): { string }
	local out: { string } = {}
	local i = 1
	local rest = false
	while i <= #args do
		local a = args[i]
		if rest or string.sub(a, 1, 1) ~= "-" or a == "-" then
			table.insert(out, a)
		elseif a == "--" then
			rest = true
		elseif string.sub(a, 1, 2) == "--" then
			if not string.find(a, "=", 1, true) and jouet.contains(long, a) then
				i += 1
			end
		else
			-- A short cluster: a value-taking letter last takes the next word.
			for at = 2, #a do
				if string.find(values, string.sub(a, at, at), 1, true) then
					if at == #a then
						i += 1
					end
					break
				end
			end
		end
		i += 1
	end
	return out
end

-- What the call would add: "staged" for a commit, "tracked" for `git commit
-- -a`, "outgoing" for a push; and the paths it, or a `git add` before it,
-- names.
local function what(call: JouetCall): (string?, { string })
	local paths: { string } = {}
	for _, command in call.commands do
		local sub, args = jouet.git(command)
		if sub == "add" then
			for _, p in operands(args, "", { "--chmod", "--pathspec-from-file" }) do
				table.insert(paths, p)
			end
		elseif sub == "commit" then
			for _, a in jouet.options(args) do
				if a == "--all" or jouet.short_flag(a, "a", COMMIT_VALUES) then
					return "tracked", paths
				end
			end
			for _, p in operands(args, COMMIT_VALUES, COMMIT_LONG) do
				table.insert(paths, p)
			end
			return "staged", paths
		elseif sub == "push" then
			return "outgoing", paths
		end
	end
	return nil, paths
end

local function list(findings: { JouetSecretFinding }): string
	local parts: { string } = {}
	for i, f in findings do
		if i > LISTED then
			table.insert(parts, `and {#findings - LISTED} more`)
			break
		end
		local at = if f.line and f.line > 0 then `{f.path}:{f.line}` else f.path
		if f.commit then
			at = `{string.sub(f.commit, 1, 12)} {at}`
		end
		table.insert(parts, `{f.rule} at {at} ({f.redacted})`)
	end
	return table.concat(parts, "; ")
end

local rule: JouetRule = {
	check = function(call: JouetCall): JouetDecision?
		local scope, paths = what(call)
		if not scope then
			return nil
		end
		local findings = jouet.secrets.scan(scope, paths)
		if not findings or #findings == 0 then
			return nil
		end
		local specific = false
		for _, f in findings do
			if string.sub(f.rule, 1, 7) ~= "generic" then
				specific = true
			end
		end
		local n = if #findings == 1 then "a secret" else `{#findings} secrets`
		local message
		if scope == "outgoing" then
			message = `These commits, not on any remote yet, add {n} gitleaks' rules recognise: {list(findings)}. Once pushed, a secret is in the history of every copy: take it out of the commits that add it (amend, or rebase them), and rotate it if it is real. {LIST_OUTGOING} lists them all; a false positive goes in .gitleaksignore as COMMIT:PATH:RULE:LINE.`
		else
			message = `This commit would add {n} gitleaks' rules recognise: {list(findings)}. Take it out and read it from the environment instead (in a fnox project, \`fnox exec -- CMD\`). \`jouet secrets scan --staged\` lists them all. For a false positive, commit with \`jouet commit PATHS -m MSG --allow-secret RULE:PATH:LINE\`, or put \`gitleaks:allow\` on its line.`
		end
		if specific then
			return jouet.deny(message)
		end
		return jouet.warn(`{message} Only gitleaks' generic rule matched, which is often wrong: check before you go on.`)
	end,
}

return rule

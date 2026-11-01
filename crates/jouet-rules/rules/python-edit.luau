--!strict
-- The `python-edit` rule's script (python-edit.toml). Its matcher finds a
-- Python run whose inline code (`-c` or a here-document) writes a file; the
-- script fires when that code also reads the same file, which makes it an
-- edit, however it changes the text: `.replace()`, slicing by `.index()`,
-- `re.sub`. Writing one file from another is not an edit, and neither is a
-- structured rewrite through json, yaml or toml, which `jouet edit` cannot do.
-- Code that checks its text is there first (an `assert`, or a `raise` or
-- `sys.exit` on a miss) cannot silently change nothing, so it is let through:
-- denying it cost the agent a whole request to make the same edit again.

-- The code a Python command runs inline: its here-documents and `-c`.
local function code_of(command: JouetCommand): string
	local parts = table.clone(command.heredocs)
	for i, arg in command.argv do
		if arg == "-c" and command.argv[i + 1] then
			table.insert(parts, command.argv[i + 1])
		end
	end
	return table.concat(parts, "\n")
end

-- How a path is written in the code: a variable, or a quoted literal, kept
-- without its quotes so `'f'` and `"f"` are one file.
local TOKENS = { "([%a_][%w_]*)", "'([^']*)'", '"([^"]*)"' }

-- Every target the code opens or reaches through pathlib in a way `kind`
-- ("read" or "write") names.
local function targets(code: string, kind: string): { [string]: boolean }
	local found: { [string]: boolean } = {}
	local function add(pattern: string)
		for token in string.gmatch(code, pattern) do
			found[token] = true
		end
	end
	for _, token in TOKENS do
		if kind == "read" then
			-- `open(p)`, or with a mode that reads.
			add("open%(%s*" .. token .. "%s*%)")
			add("open%(%s*" .. token .. "%s*,%s*['\"]r")
			add(token .. "%.read_text%(")
			add(token .. "%.read_bytes%(")
			add("Path%(%s*" .. token .. "%s*%)%.read_text%(")
			add("Path%(%s*" .. token .. "%s*%)%.read_bytes%(")
		else
			-- `open(p, 'w')`, `'a'` or `'r+'`.
			add("open%(%s*" .. token .. "%s*,%s*['\"][wa]")
			add("open%(%s*" .. token .. "%s*,%s*['\"]r%+")
			add(token .. "%.write_text%(")
			add(token .. "%.write_bytes%(")
			add("Path%(%s*" .. token .. "%s*%)%.write_text%(")
			add("Path%(%s*" .. token .. "%s*%)%.write_bytes%(")
		end
	end
	return found
end

-- Libraries that parse and write a file whole, by structure.
local STRUCTURED = { "json%.dump", "yaml%.dump", "yaml%.safe_dump", "toml%.dump", "tomlkit%.dump", "tomli_w%.dump" }

-- Whether the code stops on a miss before it writes: an `assert` line, or a
-- `raise` or `sys.exit`, as `assert old in s` or `if old not in s: raise`.
local function checked(code: string): boolean
	for line in string.gmatch(code, "[^\n]+") do
		if string.match(line, "^%s*assert%s") or string.find(line, "raise%s") or string.find(line, "sys%.exit%(") then
			return true
		end
	end
	return false
end

local function structured(code: string): boolean
	for _, pattern in STRUCTURED do
		if string.find(code, pattern) then
			return true
		end
	end
	return false
end

local rule: JouetRule = {
	check = function(call: JouetCall): JouetDecision?
		for _, command in call.commands do
			local program = command.program or ""
			if string.match(program, "^python") then
				local code = code_of(command)
				if code ~= "" and not structured(code) and not checked(code) then
					local reads = targets(code, "read")
					for target in targets(code, "write") do
						if reads[target] then
							return jouet.deny(
								"Run `jouet edit` instead. It applies exact replacements across files all-or-nothing and reports any that do not match (`jouet agent-help edit`)."
							)
						end
					end
				end
			end
		end
		return nil
	end,
}

return rule

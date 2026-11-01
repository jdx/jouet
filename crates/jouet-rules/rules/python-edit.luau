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

-- A Python string literal starting at `at` in `code`: its text with the
-- common escapes undone (none for a raw string), and where it ends; nil for
-- what this cannot read, such as an f-string.
local ESCAPES = { n = "\n", t = "\t", r = "\r", ["\\"] = "\\", ["'"] = "'", ['"'] = '"', ["\n"] = "" }
local function literal(code: string, at: number): (string?, number?)
	local prefix = string.match(code, "^[rRbBuU]*", at) or ""
	if string.find(prefix, "[fF]") then
		return nil, nil
	end
	local raw = string.find(prefix, "[rR]") ~= nil
	local i = at + #prefix
	local quote = string.sub(code, i, i + 2)
	if quote ~= '"""' and quote ~= "'''" then
		quote = string.sub(code, i, i)
		if quote ~= '"' and quote ~= "'" then
			return nil, nil
		end
	end
	local out = {}
	local j = i + #quote
	while j <= #code do
		if string.sub(code, j, j + #quote - 1) == quote then
			return table.concat(out), j + #quote
		end
		local c = string.sub(code, j, j)
		if c == "\\" and not raw then
			local e = string.sub(code, j + 1, j + 1)
			table.insert(out, ESCAPES[e] or ("\\" .. e))
			j += 2
		elseif c == "\n" and #quote == 1 then
			return nil, nil
		else
			table.insert(out, c)
			j += 1
		end
	end
	return nil, nil
end

-- The old text of every `.replace(old, new)` in the code, each a plain
-- literal; nil when one is not (a variable, a concatenation, an f-string),
-- since then what it matches cannot be told.
local function replace_olds(code: string): { string }?
	local olds = {}
	local from = 1
	while true do
		local s, e = string.find(code, "%.replace%(%s*", from)
		if not s then
			break
		end
		local text, after = literal(code, e + 1)
		if not text or not after or not string.match(code, "^%s*,", after) then
			return nil
		end
		table.insert(olds, text)
		from = after
	end
	return olds
end

-- The path a target names: a literal as it is, or a variable's value where
-- the code assigns it one literal (`p = 'src/a.rs'`, `p = Path("src/a.rs")`).
local function path_of(code: string, target: string): string?
	if not string.match(target, "^[%a_][%w_]*$") then
		return target
	end
	for line in string.gmatch(code, "[^\n]+") do
		local value = string.match(line, "^%s*" .. target .. "%s*=%s*['\"]([^'\"]+)['\"]%s*$")
			or string.match(line, "^%s*" .. target .. "%s*=%s*Path%(%s*['\"]([^'\"]+)['\"]%s*%)%s*$")
		if value then
			return value
		end
	end
	return nil
end

-- Whether every `.replace(` old text is in the files the code edits: true
-- or false with the first one missing, from `jouet.file.contains`; nil when
-- that cannot be told, and the edit is judged as before.
local function olds_found(code: string, targets: { string }, dir: string | false | nil): (boolean?, string?, string?)
	local olds = replace_olds(code)
	if not olds or #olds == 0 or #targets == 0 or dir == false then
		return nil, nil, nil
	end
	local paths = {}
	for _, target in targets do
		local path = path_of(code, target)
		if not path or string.sub(path, 1, 1) == "~" then
			return nil, nil, nil
		end
		if dir and string.sub(path, 1, 1) ~= "/" then
			path = `{dir}/{path}`
		end
		table.insert(paths, path)
	end
	for _, old in olds do
		local found = false
		for _, path in paths do
			local yes, err = jouet.file.contains(path, old)
			if err or yes == nil then
				return nil, nil, nil
			end
			if yes then
				found = true
				break
			end
		end
		if not found then
			return false, old, paths[1]
		end
	end
	return true, nil, nil
end

-- What a message shows of the text: its first line, cut to 60 characters.
local function shown(text: string): string
	local first = string.match(text, "^[^\n]*") or ""
	if #first > 60 or #first < #text then
		return string.sub(first, 1, 60) .. "…"
	end
	return first
end

local rule: JouetRule = {
	check = function(call: JouetCall): JouetDecision?
		local dirs = jouet.dirs(call.commands)
		for i, command in call.commands do
			local program = command.program or ""
			if string.match(program, "^python") then
				local code = code_of(command)
				if code ~= "" and not structured(code) and not checked(code) then
					local reads = targets(code, "read")
					local edited = {}
					for target in targets(code, "write") do
						if reads[target] then
							table.insert(edited, target)
						end
					end
					if #edited > 0 then
						-- Replacements whose old text is there land; one whose
						-- text is not would change nothing, so say which.
						local found, missing, path = olds_found(code, edited, dirs[i])
						if found == false and missing then
							return jouet.deny(
								`The \`.replace(\` text \`{shown(missing)}\` is not in {path}, so this script would change nothing there. Fix the text, or run \`jouet edit\`, which reports any replacement that does not match.`
							)
						elseif found ~= true then
							return jouet.deny(
								"Run `jouet edit` instead. It applies exact replacements across files all-or-nothing and reports any that do not match (`jouet agent-help edit`). To add code, replace a line next to where it goes with that line plus the new code."
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

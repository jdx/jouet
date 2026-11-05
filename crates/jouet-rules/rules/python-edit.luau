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

-- Whether the code stops on a miss before it writes: an `assert` statement,
-- on its own line or after a `;` (`s=open(p).read(); assert a in s`), or a
-- `raise` or `sys.exit`, as `assert old in s` or `if old not in s: raise`;
-- or it finds its place by `.index(` (or `.rindex(`), which raises on a miss,
-- and changes no text by `.replace(` or `re.sub`, which would not.
local function checked(code: string): boolean
	for line in string.gmatch(code, "[^\n]+") do
		if string.match(line, "^%s*assert%s") or string.find(line, ";%s*assert%s") or string.find(line, "raise%s") or string.find(line, "sys%.exit%(") then
			return true
		end
	end
	return string.find(code, "%.r?index%(") ~= nil
		and not string.find(code, "%.replace%(")
		and not string.find(code, "re%.subn?%(")
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

-- The code with every string literal's text and every comment blanked out,
-- the same length, so a search finds only Python: not a `p = ` or a
-- `.replace(` in the Go or JavaScript a script writes.
local function blanked(code: string): string
	local out = {}
	local i = 1
	while i <= #code do
		local c = string.sub(code, i, i)
		local _, after = nil, nil
		if c == "'" or c == "\"" then
			_, after = literal(code, i)
		end
		if after then
			table.insert(out, c .. string.rep(" ", after - i - 2) .. c)
			i = after
		elseif c == "#" then
			local stop = string.find(code, "\n", i, true) or #code + 1
			table.insert(out, string.rep(" ", stop - i))
			i = stop
		else
			table.insert(out, c)
			i += 1
		end
	end
	return table.concat(out)
end

-- Where each statement in the code starts: a line's start, or just after a
-- `;` that runs two statements on one line (`p='a.go'; s=open(p).read()`).
local function statements(python: string): { number }
	local starts = { 1 }
	for at in string.gmatch(python, "[\n;]()") do
		table.insert(starts, at)
	end
	return starts
end

-- Every literal the code assigns to `name` (`name = '...'`, `name = """..."""`,
-- `name = Path('...')`) and where, in order; nil when one is not a literal,
-- since then what it holds cannot be told.
local function values_of(code: string, name: string): { { text: string, at: number } }?
	local values = {}
	local python = blanked(code)
	for _, at in statements(python) do
		local _, e = string.find(python, "^[ \t]*" .. name .. "%s*=%s*", at)
		if e and string.sub(code, e + 1, e + 1) ~= "=" then
			local path = string.match(code, "^Path%(%s*()", e + 1)
			local text = literal(code, path or e + 1)
			if not text then
				return nil
			end
			table.insert(values, { text = text, at = at })
		end
	end
	table.sort(values, function(a, b)
		return a.at < b.at
	end)
	return values
end

-- The old text of every `.replace(old, new)` in the code, each a plain
-- literal or a variable, by the literal last assigned it before the call
-- (`marker = '...'`); nil when one is not (a concatenation, an f-string, a loop variable),
-- since then what it matches cannot be told. Each keeps its new text too
-- when that is a literal, since a later replacement can match what it inserts.
local function replace_olds(code: string): { { text: string, new: string? } }?
	local olds: { { text: string, new: string? } } = {}
	local python = blanked(code)
	local from = 1
	while true do
		local s, e = string.find(python, "%.replace%(%s*", from)
		if not s or not e then
			break
		end
		local text, after = literal(code, e + 1)
		if not text then
			local name, past = string.match(code, "^([%a_][%w_]*)()", e + 1)
			local values = if name then values_of(code, name) else nil
			for _, value in ipairs(values or {}) do
				if value.at < s then
					text = value.text
				end
			end
			if not text then
				return nil
			end
			after = past
		end
		if not after or not string.match(code, "^%s*,", after) then
			return nil
		end
		if not text then
			return nil
		end
		local start = string.match(code, "^%s*,%s*()", after)
		local new = if start then literal(code, start) else nil
		table.insert(olds, { text = text, new = new })
		from = after
	end
	return olds
end

-- The paths a target names: a literal as it is, or each literal the code
-- assigns the variable, as a script that edits files in turn does
-- (`p = 'a.go'` ... `p = 'a_test.go'`); nil when one is not a literal.
local function paths_of(code: string, target: string): { string }?
	if not string.match(target, "^[%a_][%w_]*$") then
		return { target }
	end
	local values = values_of(code, target)
	if not values or #values == 0 then
		return nil
	end
	local paths = {}
	for _, value in values do
		table.insert(paths, value.text)
	end
	return paths
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
		local named = paths_of(code, target)
		if not named then
			return nil, nil, nil
		end
		for _, path in named do
			if string.sub(path, 1, 1) == "~" then
				return nil, nil, nil
			end
			if dir and string.sub(path, 1, 1) ~= "/" then
				path = `{dir}/{path}`
			end
			table.insert(paths, path)
		end
	end
	for i, replacement in olds do
		local old = replacement.text
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
			-- Text an earlier replacement inserts, as `a` to `a\nb` then `b`.
			for j = 1, i - 1 do
				local new = olds[j].new
				if new and string.find(new, old, 1, true) then
					found = true
					break
				end
			end
		end
		if not found then
			return false, old, table.concat(paths, " or ")
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

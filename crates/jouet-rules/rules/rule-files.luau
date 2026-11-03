--!strict
-- The `rule-files` rule's script (rule-files.toml). The matcher finds a
-- write to a file in a jouet rules directory; the script works out which
-- files the call writes, and reads each to see whether it holds a rule in
-- deny mode that another author wrote. A file it cannot read is a new one,
-- which only adds decisions.

-- The ways a TOML or Luau file can write `key = "value"`.
local function forms(key: string, value: string): { string }
	return { `{key} = "{value}"`, `{key}="{value}"`, `{key} = '{value}'`, `{key}='{value}'` }
end

local DENY = forms("mode", "deny")

-- Options that take a value, so the value is not a path.
local SED_VALUES = { "-e", "--expression", "-f", "--file", "-l", "--line-length" }
local TRUNCATE_VALUES = { "-s", "--size", "-r", "--reference" }
local INSTALL_VALUES =
	{ "-m", "--mode", "-o", "--owner", "-g", "--group", "-S", "--suffix", "-t", "--target-directory" }
local JOUET_EDIT_VALUES = {
	"--old",
	"--new",
	"--count",
	"--check",
	"--spec",
	"--format",
	"--pattern",
	"--rewrite",
	"--lang",
	"--callers-of",
	"--impact",
	"--expect-hash",
}

-- The arguments that are not options or the values of `values`.
local function positional(args: { string }, values: { string }): { string }
	local out = {}
	local skip, ended = false, false
	for _, arg in args do
		if skip then
			skip = false
		elseif ended or string.sub(arg, 1, 1) ~= "-" then
			if arg ~= "" then
				table.insert(out, arg)
			end
		elseif arg == "--" then
			ended = true
		elseif jouet.contains(values, arg) then
			skip = true
		end
	end
	return out
end

local function rest(argv: { string }, from: number): { string }
	local out = {}
	for i = from, #argv do
		table.insert(out, argv[i])
	end
	return out
end

-- "repo" for a repository's `.jouet/rules`, "user" for any other jouet rules
-- directory (the user's or the system's), and whether the path is the
-- directory itself rather than a file in it.
local function place(path: string): (string?, boolean)
	local p = "/" .. (string.gsub(path, "\\", "/"))
	local at = string.find(p, "/%.?jouet/rules/") or string.find(p, "/%.?jouet/rules$")
	if not at then
		return nil, false
	end
	local kind = if string.sub(p, at + 1, at + 1) == "." then "repo" else "user"
	local dir = string.find(p, "jouet/rules/?$") ~= nil
	return kind, dir
end

-- The directory GNU's `-t DIR`, `-tDIR` or `--target-directory[=]DIR` names.
local function target_dir(args: { string }): string?
	for i, arg in args do
		if arg == "--" or string.sub(arg, 1, 1) ~= "-" then
			if arg == "--" then
				return nil
			end
		elseif arg == "--target-directory" then
			return args[i + 1]
		elseif string.sub(arg, 1, 19) == "--target-directory=" then
			return string.sub(arg, 20)
		elseif string.sub(arg, 1, 2) ~= "--" and string.sub(arg, 1, 2) == "-t" and #arg > 2 then
			return string.sub(arg, 3)
		elseif string.sub(arg, 1, 2) ~= "--" and string.sub(arg, -1) == "t" then
			-- `-t` alone, or at the end of a cluster such as `-rt`.
			return args[i + 1]
		end
	end
	return nil
end

-- Whether a path's last part could name every rule file, or a rule file
-- the shell finds: `.`, `..`, or a pattern such as `*` or `*.toml`, but not
-- one that names another kind of file, such as `*.md`.
local function broad(path: string): boolean
	local name = string.match(path, "([^/\\]*)$") or path
	if name == "." or name == ".." then
		return true
	end
	if not string.find(name, "[%*%?%[{]") then
		return false
	end
	local ext = string.match(name, "%.(%w+)$")
	return ext == nil or ext == "toml" or ext == "luau"
end

-- What copying or moving `sources` to `dest` writes: `dest`, or the file of
-- each source's name in it when it is a rules directory.
local function into(dest: string, sources: { string }): { string }
	local _, dir = place(dest)
	if not dir then
		return { dest }
	end
	local out = {}
	for _, source in sources do
		table.insert(out, `{string.gsub(dest, "/$", "")}/{jouet.basename(source)}`)
	end
	return out
end

-- The files a command writes, moves or deletes, as written.
local function written(command: JouetCommand): { string }
	local out: { string } = {}
	if command.stdout_to then
		table.insert(out, command.stdout_to)
	end
	if not command.matched then
		return out
	end
	local argv, program = command.argv, command.program
	local found: { string } = {}
	if program == "cp" or program == "install" or program == "mv" then
		local args = rest(argv, 2)
		local named = positional(args, INSTALL_VALUES)
		-- GNU's `-t DIR` names the destination, and every argument is a source.
		local target = target_dir(args)
		local dest = target or table.remove(named)
		if target then
			-- A cluster such as `-rt DIR` leaves DIR among the arguments.
			local kept = {}
			for _, arg in named do
				if arg ~= target then
					table.insert(kept, arg)
				end
			end
			named = kept
		end
		if dest and #named > 0 then
			found = into(dest, named)
			-- A move takes its sources away too.
			if program == "mv" then
				for _, source in named do
					table.insert(found, source)
				end
			end
		end
	elseif program == "sed" then
		local args = rest(argv, 2)
		local named = positional(args, SED_VALUES)
		local scripted = false
		for _, arg in jouet.options(args) do
			scripted = scripted or jouet.contains(SED_VALUES, arg)
		end
		-- Without `-e` or `-f`, the first argument is the script.
		if not scripted then
			table.remove(named, 1)
		end
		found = named
	elseif program == "jouet" then
		if argv[2] == "edit" then
			found = positional(rest(argv, if argv[3] == "replace" then 4 else 3), JOUET_EDIT_VALUES)
		end
	elseif program == "truncate" then
		found = positional(rest(argv, 2), TRUNCATE_VALUES)
	elseif program == "rm" or program == "unlink" or program == "shred" or program == "tee" then
		found = positional(rest(argv, 2), {})
	end
	for _, path in found do
		table.insert(out, path)
	end
	return out
end

local function holds_any(path: string, texts: { string }): boolean
	for _, text in texts do
		if jouet.file.contains(path, text) then
			return true
		end
	end
	return false
end

-- Whether the file is a rule in deny mode that `session` did not write. A
-- script's mode and author are in the TOML file beside it.
local function guarded(path: string, session: string?): boolean
	local files = { path }
	if string.sub(path, -5) == ".luau" then
		table.insert(files, string.sub(path, 1, -6) .. ".toml")
	elseif string.sub(path, -5) ~= ".toml" then
		return false
	end
	for _, file in files do
		if holds_any(file, DENY) then
			local own = session ~= nil and holds_any(file, forms("author", session))
			return not own
		end
	end
	return false
end

local rule: JouetRule = {
	check = function(call: JouetCall): JouetDecision?
		local paths: { string } = {}
		if call.path then
			table.insert(paths, call.path)
		end
		for _, command in call.commands do
			for _, path in written(command) do
				table.insert(paths, path)
			end
		end
		local repo: string? = nil
		for _, path in paths do
			local kind, dir = place(path)
			if kind == "user" and (dir or broad(path)) then
				return jouet.deny(
					`{path} holds the user's jouet rules, which steer you; moving or deleting it switches them off, which is the user's decision: ask them. To add a restriction, write a rule of your own with \`jouet rule new NAME --why TEXT\`.`
				)
			elseif kind == "user" and guarded(path, call.session) then
				return jouet.deny(
					`{path} holds a jouet rule in deny mode, which steers you; changing, moving or deleting it can switch that off, which is the user's decision: ask them. To add a restriction, write a rule of your own with \`jouet rule new NAME --why TEXT\`; rules in warn mode, and those this session wrote, are yours to edit.`
				)
			elseif kind == "repo" then
				repo = repo or path
			end
		end
		if repo then
			return jouet.warn(
				`{repo} is one of this repository's jouet rules. Once the user has trusted them, changing anything in .jouet/rules turns off every rule and routine there until they review the change and run \`jouet trust\`; tell them, and keep a change to a rule for a reviewed PR.`
			)
		end
		return nil
	end,
}

return rule

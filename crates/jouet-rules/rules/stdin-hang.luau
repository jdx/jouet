--!strict
-- The `stdin-hang` rule's script (stdin-hang.toml). Its matchers find `cat`
-- and Python with no file or with `-`; the script fires when one reads standard input that nothing
-- feeds: no here-document, `<` or `<<<`, and no pipe into it. Claude Code
-- leaves that input open, so the read never ends and the call runs until its
-- timeout. Codex gives a command no input, so the read ends at once there.

local MESSAGE =
	"It reads standard input, which nothing writes to here, so the call would hang until it times out: give it a here-document (`cat > FILE <<'EOF'`), a pipe or a file, or drop it."

-- Whether a command reads its standard input: `cat` with no file but `-`,
-- or Python with no script, `-c` or `-m`.
local function reads_stdin(command: JouetCommand): boolean
	local argv = command.argv
	if command.program == "cat" then
		for i = 2, #argv do
			if argv[i] == "-" or string.sub(argv[i], 1, 1) ~= "-" then
				return argv[i] == "-"
			end
		end
		return true
	end
	for i = 2, #argv do
		local arg = argv[i]
		if arg == "-" then
			return true
		elseif arg == "-c" or arg == "-m" or string.sub(arg, 1, 1) ~= "-" or string.sub(arg, 1, 2) == "--" then
			-- A script, inline code, a module, or `--version` and the like.
			return false
		end
	end
	return true
end

local rule: JouetRule = {
	check = function(call: JouetCall): JouetDecision?
		if call.harness == "codex" then
			return nil
		end
		for i, command in call.commands do
			local piped = i > 1 and call.commands[i - 1].op == "pipe"
			if command.matched and not piped and not command.stdin_redirected and reads_stdin(command) then
				return jouet.deny(MESSAGE)
			end
		end
		return nil
	end,
}

return rule

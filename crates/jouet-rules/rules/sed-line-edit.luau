--!strict
-- The `sed-line-edit` rule's script (sed-line-edit.toml). Its matcher finds
-- `sed -i`; the script fires when one of its commands deletes, changes or
-- inserts by line number (`356,413d`, `632,635c\`, `12i\`), which edits
-- whatever those lines hold by then, whether or not it is the text meant.
-- A substitution (`12s/a/b/`) checks its pattern, and a pattern address
-- (`/^fn a/d`) names the text, so both are left alone.

-- Whether a sed command line is a line-number edit: an address of one
-- number, or a range from a number, then `d`, `c`, `i` or `a`.
local function line_edit(expr: string): boolean
	for part in string.gmatch(expr .. ";", "([^;\n]+)[;\n]") do
		local address, command = string.match(part, "^%s*(%d+[%s,%d%$+~]*)%s*([dcia])")
		if address and command then
			return true
		end
	end
	return false
end

-- The scripts sed runs: each `-e` or `--expression`, else its first word
-- that is not an option.
local function scripts(argv: { string }): { string }
	local out: { string } = {}
	local first: string? = nil
	local i = 2
	while i <= #argv do
		local arg = argv[i]
		if arg == "-e" or arg == "--expression" then
			if argv[i + 1] then
				table.insert(out, argv[i + 1])
			end
			i += 2
		elseif string.sub(arg, 1, 13) == "--expression=" then
			table.insert(out, string.sub(arg, 14))
			i += 1
		elseif arg == "-f" or arg == "--file" then
			i += 2
		elseif string.sub(arg, 1, 1) == "-" and #arg > 1 then
			i += 1
		else
			first = first or arg
			i += 1
		end
	end
	if #out == 0 and first then
		table.insert(out, first)
	end
	return out
end

-- Whether sed edits its files in place: `-i`, `-i.bak`, `-Ei` or
-- `--in-place`.
local function in_place(argv: { string }): boolean
	for i = 2, #argv do
		local arg = argv[i]
		if arg == "--in-place" or string.sub(arg, 1, 11) == "--in-place=" or jouet.short_flag(arg, "i", "ef") then
			return true
		end
	end
	return false
end

local rule: JouetRule = {
	check = function(call: JouetCall): JouetDecision?
		for _, command in call.commands do
			if command.program == "sed" and in_place(command.argv) then
				for _, expr in scripts(command.argv) do
					if line_edit(expr) then
						return jouet.warn(
							"Edit by text, with your edit tool or `jouet edit replace FILE --old TEXT --new TEXT`, which fails when the text is not there. A `sed -i` that deletes, changes or inserts by line number edits whatever those lines hold by then: once one edit lands, every line number below it is off."
						)
					end
				end
			end
		end
		return nil
	end,
}

return rule

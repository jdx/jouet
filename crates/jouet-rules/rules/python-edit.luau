--!strict
--- name: python-edit
--- why: A Python heredoc that calls `.replace(` and writes the file back never checks that the text matched; `jouet edit` does.
--- mode: deny
--- scope: builtin
--- author: builtin
--- created: 2026-10-30

local function is_python(program: string?): boolean
	if not program or string.sub(program, 1, 6) ~= "python" then
		return false
	end
	return string.match(string.sub(program, 7), "^[%d%.]*$") ~= nil
end

local function edits(code: string): boolean
	return string.find(code, ".replace(", 1, true) ~= nil
		and (
			string.find(code, "write(", 1, true) ~= nil
			or string.find(code, "write_text(", 1, true) ~= nil
			or string.find(code, "write_bytes(", 1, true) ~= nil
		)
end

local rule: JouetRule = {
	needs = { "edit" },
	check = function(call: JouetCall): JouetDecision?
		for _, command in call.commands do
			if is_python(command.program) then
				local argv = command.argv
				for i, arg in argv do
					if arg == "-c" and argv[i + 1] and edits(argv[i + 1]) then
						return jouet.deny(
							"Use `jouet edit` instead of a Python rewrite: it applies exact replacements across files all-or-nothing and reports any that do not match (`jouet agent-help edit`)."
						)
					end
				end
				for _, code in command.heredocs do
					if edits(code) then
						return jouet.deny(
							"Use `jouet edit` instead of a Python rewrite: it applies exact replacements across files all-or-nothing and reports any that do not match (`jouet agent-help edit`)."
						)
					end
				end
			end
		end
		return nil
	end,
	examples = {
		fires = {
			"python3 - <<'EOF'\nfrom pathlib import Path\np = Path('a.rs')\np.write_text(p.read_text().replace('a', 'b'))\nEOF",
			"python3 -c \"s=open('f').read().replace('x','y'); open('f','w').write(s)\"",
			"cd src && python3.12 <<EOF\nt = open('x').read()\nt = t.replace('a', 'b')\nopen('x', 'w').write(t)\nEOF",
		},
		quiet = {
			"python3 - <<'EOF'\nprint(open('f').read().replace('a', 'b'))\nEOF",
			"python3 fix.py",
			"cat <<'EOF' > fix.py\nopen('f','w').write(s.replace('a','b'))\nEOF",
			"pythonic --replace( write(",
		},
	},
}

return rule

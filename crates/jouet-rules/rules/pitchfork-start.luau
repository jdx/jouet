--!strict
-- The `pitchfork-start` rule's script (pitchfork-start.toml). In a project
-- with a pitchfork config, it reads the daemons the config declares
-- (`jouet.pitchfork.daemons()`) and denies starting one of them by hand: a
-- command that is a daemon's `run` command (with arguments added after it,
-- or not), or a server told to listen on a daemon's port. It names the
-- daemon to `pitchfork start` instead.
--
-- Without the probe (as in the rule's examples) it cannot know the daemons,
-- and judges a dev server started by hand on its shape alone.

-- Options that give the port to listen on, alone or as HOST:PORT.
local PORT_FLAGS: { [string]: boolean } = {
	["--port"] = true,
	["-p"] = true,
	["--bind"] = true,
	["-b"] = true,
	["--listen"] = true,
	["-l"] = true,
}

-- Programs whose commands are never a daemon started by hand.
local SKIP: { [string]: boolean } = {
	pitchfork = true,
	jouet = true,
	joue = true,
	cd = true,
	echo = true,
	sleep = true,
	kill = true,
	pkill = true,
}

local function words(text: string): { string }
	local out = {}
	for word in string.gmatch(text, "%S+") do
		table.insert(out, word)
	end
	return out
end

-- A daemon's `run` as the words of the command that runs in the end: the
-- last part of `a && b; c`, without a leading `exec`.
local function run_words(run: string): { string }
	local last = run
	for part in string.gmatch(run .. ";", "([^;&|]+)[;&|]+") do
		if string.find(part, "%S") then
			last = part
		end
	end
	local out = words(last)
	if out[1] == "exec" then
		table.remove(out, 1)
	end
	return out
end

-- Whether `argv` starts with `run`: the same command, maybe with arguments
-- added. A one-word `run` must match exactly.
local function same(argv: { string }, run: { string }): boolean
	if #run == 0 or #argv < #run or (#run == 1 and #argv ~= 1) then
		return false
	end
	if jouet.basename(argv[1]) ~= jouet.basename(run[1]) then
		return false
	end
	for i = 2, #run do
		if argv[i] ~= run[i] then
			return false
		end
	end
	return true
end

local function port_of(text: string?): number?
	if not text then
		return nil
	end
	local port = tonumber(string.match(text, "^(%d+)$") or string.match(text, ":(%d+)$") or "")
	if port and port > 0 and port < 65536 then
		return port
	end
	return nil
end

-- The port the command tells a server to listen on.
local function given_port(argv: { string }): number?
	for i, arg in argv do
		local flag, value = string.match(arg, "^(%-%-[%w%-]+)=(.+)$")
		if flag and PORT_FLAGS[flag] then
			return port_of(value)
		elseif PORT_FLAGS[arg] then
			local port = port_of(argv[i + 1])
			if port then
				return port
			end
		end
	end
	return nil
end

local function shown(argv: { string }): string
	return table.concat(argv, " ")
end

local function advice(name: string, why: string): JouetDecision
	return jouet.deny(
		`{why}: run \`pitchfork start {name}\` instead. It starts the declared daemon and returns once its ready check passes, where a copy started by hand runs without pitchfork's log, ready check or restarts, and may take its port. Then \`pitchfork logs {name} -n 50\` reads its log, \`jouet wait daemon {name}\` waits for it after a restart, and \`pitchfork restart {name}\` or \`pitchfork stop {name}\` replace killing it.`
	)
end

-- A dev server's shape, for when the daemons cannot be read.
local function looks_like_a_server(argv: { string }): boolean
	local program = jouet.basename(argv[1] or "")
	local script = if argv[2] == "run" then argv[3] else argv[2]
	if program == "npm" or program == "pnpm" or program == "yarn" or program == "bun" then
		return script ~= nil and string.match(script, "^dev") ~= nil
	end
	return program == "vite" or program == "uvicorn" or (program == "python3" and jouet.contains(argv, "http.server"))
end

local rule: JouetRule = {
	check = function(call: JouetCall): JouetDecision?
		local daemons, err = jouet.pitchfork.daemons()
		for _, c in call.commands do
			local argv = c.argv
			local program = c.program or ""
			if c.nested or #argv == 0 or SKIP[program] then
				continue
			end
			if err then
				if looks_like_a_server(argv) then
					return advice("ID", `If pitchfork.toml declares \`{shown(argv)}\` as a daemon`)
				end
				continue
			end
			if not daemons then
				return nil
			end
			local port = given_port(argv)
			for _, d in daemons do
				if same(argv, run_words(d.run)) then
					return advice(d.name, `\`{shown(argv)}\` is the \`{d.name}\` daemon in {jouet.basename(d.file)}`)
				end
				if port and table.find(d.ports, port) then
					return advice(d.name, `\`{shown(argv)}\` listens on port {port}, the \`{d.name}\` daemon's in {jouet.basename(d.file)}`)
				end
			end
		end
		return nil
	end,
}

return rule

--!strict
-- The `pitchfork-stop` rule's script (pitchfork-stop.toml). Its matchers
-- found a kill by port or by command line; in a project with a pitchfork
-- config, this denies it when what it kills is one of the declared daemons:
-- a port one listens on (`lsof -i :3000`, `fuser -k 3000/tcp`, `kill-port
-- 3000`), or a pattern in one's `run` command (`pkill -f 'npm run dev'`).
-- Anything else is left to `job-stop`.
--
-- Without the probe (as in the rule's examples) it cannot know the daemons,
-- and warns that a pitchfork daemon is stopped by name.

-- The ports a command names: `:3000`, `-i:3000`, `3000/tcp`, or a bare
-- number for `kill-port`.
local function ports(c: JouetCommand): { number }
	local out = {}
	local bare = c.program == "kill-port" or jouet.contains(c.argv, "kill-port")
	for i = 2, #c.argv do
		local arg = c.argv[i]
		local digits = string.match(arg, ":(%d+)$")
			or string.match(arg, "^(%d+)/tcp$")
			or (if bare then string.match(arg, "^(%d+)$") else nil)
		local port = tonumber(digits or "")
		if port then
			table.insert(out, port)
		end
	end
	return out
end

-- The patterns `pkill` and `killall` are given: their arguments.
local function patterns(c: JouetCommand): { string }
	local out = {}
	if c.program ~= "pkill" and c.program ~= "killall" then
		return out
	end
	for i = 2, #c.argv do
		local arg = c.argv[i]
		if string.sub(arg, 1, 1) ~= "-" then
			table.insert(out, arg)
		end
	end
	return out
end

local function advice(name: string, why: string): JouetDecision
	return jouet.deny(
		`{why}: run \`pitchfork stop {name}\` instead, or \`pitchfork restart {name}\` to start it again. It stops the daemon's whole process group, gracefully and then by force, and keeps its log, where a kill looks to pitchfork like a crash it may restart.`
	)
end

local rule: JouetRule = {
	check = function(call: JouetCall): JouetDecision?
		local daemons, err = jouet.pitchfork.daemons()
		if err then
			return jouet.warn(
				"If this is a pitchfork daemon, `pitchfork stop ID` stops it by name, gracefully and then by force, and keeps its log; `pitchfork list --project` names them."
			)
		end
		if not daemons then
			return nil
		end
		for _, c in call.commands do
			for _, port in ports(c) do
				for _, d in daemons do
					if table.find(d.ports, port) then
						return advice(d.name, `Port {port} is the \`{d.name}\` daemon's`)
					end
				end
			end
			for _, pattern in patterns(c) do
				for _, d in daemons do
					if string.find(d.run, pattern, 1, true) then
						return advice(d.name, `\`{pattern}\` matches the \`{d.name}\` daemon's command`)
					end
				end
			end
		end
		return nil
	end,
}

return rule

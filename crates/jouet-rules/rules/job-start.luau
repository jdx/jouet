--!strict
-- The `job-start` rule's script (job-start.toml). Its matchers have found a
-- server, a watcher, a build or a test suite started in the background; this
-- names the `jouet job start` that replaces it. For a server, the ready
-- check comes from what the call shows: the local URL it goes on to poll, a
-- port the server is told to listen on, or the port it listens on by
-- default; without one, it says how to add one. A build or a test suite
-- needs none: `jouet wait job` waits for it to end.

-- The port each kind of server listens on when it is not told otherwise.
local DEFAULT_PORTS: { [string]: number } = {
	["http.server"] = 8000,
	runserver = 8000,
	uvicorn = 8000,
	gunicorn = 8000,
	mkdocs = 8000,
	artisan = 8000,
	flask = 5000,
	vite = 5173,
	vitepress = 5173,
	next = 3000,
	nuxt = 3000,
	nuxi = 3000,
	rails = 3000,
	astro = 4321,
	hugo = 1313,
	jekyll = 4000,
	["http-server"] = 8080,
	trunk = 8080,
	storybook = 6006,
	ng = 4200,
	wrangler = 8787,
}

-- Options that give the port to listen on, alone or as HOST:PORT.
local PORT_FLAGS: { [string]: boolean } = {
	["--port"] = true,
	["-p"] = true,
	["--bind"] = true,
	["-b"] = true,
	["--listen"] = true,
	["-l"] = true,
	["-S"] = true,
}

local PACKAGE_MANAGERS: { [string]: boolean } = { npm = true, pnpm = true, yarn = true, bun = true }
local RUNNERS: { [string]: boolean } = { npx = true, bunx = true, pnpx = true }
-- Programs that are not the server, when several run in the background, or
-- that only read a server's output through a pipe.
local NOT_SERVERS: { [string]: boolean } = {
	sleep = true,
	cd = true,
	wait = true,
	tail = true,
	cat = true,
	tee = true,
	head = true,
	grep = true,
}

local LOCAL_HOSTS = { "localhost", "127%.0%.0%.1", "0%.0%.0%.0", "%[::1%]" }

local function slice(list: { string }, from: number): { string }
	local out = {}
	for i = from, #list do
		table.insert(out, list[i])
	end
	return out
end

-- A job ID: lowercase letters, digits, `_`, `.` and single dashes, without
-- a dash at either end.
local function slug(text: string): string
	local s = string.lower(text)
	s = string.gsub(s, "%.[cm]?[jt]sx?$", "")
	s = string.gsub(s, "[^%w_.]+", "-")
	s = string.gsub(s, "%-%-+", "-")
	s = string.gsub(s, "^[-.]+", "")
	s = string.gsub(s, "[-.]+$", "")
	if s == "" then
		return "server"
	end
	return s
end

-- The daemon's name, and the kind of server it is, for its default port.
local function name_of(argv: { string }): (string, string)
	local program = jouet.basename(argv[1] or "")
	if RUNNERS[program] then
		return name_of(slice(argv, 2))
	end
	if program == "pnpm" and (argv[2] == "exec" or argv[2] == "dlx") then
		return name_of(slice(argv, 3))
	end
	if program == "bundle" and argv[2] == "exec" then
		return name_of(slice(argv, 3))
	end
	if PACKAGE_MANAGERS[program] or program == "deno" or program == "mise" then
		local i = 2
		if argv[i] == "run" or argv[i] == "run-script" or argv[i] == "task" then
			i += 1
		end
		local script = argv[i]
		if script and string.sub(script, 1, 1) ~= "-" then
			return slug(script), "script"
		end
		return slug(program), "script"
	end
	if string.match(program, "^python") then
		for i, arg in argv do
			if arg == "-m" and argv[i + 1] then
				return slug((string.gsub(argv[i + 1], "%.", "-"))), argv[i + 1]
			end
		end
		if jouet.contains(argv, "runserver") then
			return "runserver", "runserver"
		end
		return slug(jouet.basename(argv[2] or program)), program
	end
	if program == "php" then
		if argv[2] == "artisan" then
			return "artisan", "artisan"
		end
		return "php", "php"
	end
	if program == "docker" or program == "docker-compose" then
		return "compose", "compose"
	end
	if program == "cargo" and argv[2] then
		return slug(`cargo-{argv[2]}`), "cargo"
	end
	if program == "node" or program == "bun" or program == "tsx" or program == "ts-node" then
		for _, arg in slice(argv, 2) do
			if string.sub(arg, 1, 1) ~= "-" then
				return slug(jouet.basename(arg)), program
			end
		end
	end
	return slug(program), program
end

-- A port, alone or at the end of HOST:PORT.
local function port_of(text: string?): number?
	if not text then
		return nil
	end
	local digits = string.match(text, "^(%d+)$") or string.match(text, ":(%d+)$")
	local port = tonumber(digits or "")
	if port and port > 0 and port < 65536 then
		return port
	end
	return nil
end

-- The port the command tells the server to listen on.
local function given_port(argv: { string }): number?
	for i, arg in argv do
		local flag, value = string.match(arg, "^(%-%-[%w%-]+)=(.+)$")
		if flag and PORT_FLAGS[flag] then
			local port = port_of(value)
			if port then
				return port
			end
		elseif PORT_FLAGS[arg] then
			local port = port_of(argv[i + 1])
			if port then
				return port
			end
		elseif string.match(arg, "^%-p%d+$") then
			return port_of(string.sub(arg, 3))
		end
	end
	-- `http.server 8000`, `runserver 0.0.0.0:8001`: the word after.
	for i, arg in argv do
		if arg == "http.server" or arg == "runserver" then
			return port_of(argv[i + 1])
		end
	end
	return nil
end

local function is_local(url: string): boolean
	local rest = string.gsub(url, "^https?://", "")
	for _, host in LOCAL_HOSTS do
		if string.match(rest, `^{host}[:/]`) or string.match(rest, `^{host}$`) then
			return true
		end
	end
	return false
end

-- The local URL another command in the call fetches, as a full URL.
local function polled_url(commands: { JouetCommand }): string?
	for _, c in commands do
		local program = c.program
		if program == "curl" or program == "wget" or program == "http" then
			for i = 2, #c.argv do
				local arg = c.argv[i]
				if string.sub(arg, 1, 1) ~= "-" and is_local(arg) then
					if string.match(arg, "^https?://") then
						return arg
					end
					return `http://{arg}`
				end
			end
		end
	end
	return nil
end

-- A word as a POSIX shell needs it written.
local function quote(word: string): string
	if word ~= "" and not string.find(word, "[^%w%-_./=:,@%%+]") then
		return word
	end
	return "'" .. string.gsub(word, "'", "'\\''") .. "'"
end

local function shell_words(argv: { string }): string
	local out = {}
	for _, word in argv do
		table.insert(out, quote(word))
	end
	return table.concat(out, " ")
end

-- The command the call starts in the background: one followed by `&`
-- that is not a sleep or a tail, else (when the harness backgrounds the whole
-- call) its last command that is not a `cd`.
local function server(commands: { JouetCommand }): (JouetCommand?, boolean)
	for _, c in commands do
		if c.op == "background" and not c.nested and not NOT_SERVERS[c.program or ""] then
			return c, true
		end
	end
	local last: JouetCommand? = nil
	for _, c in commands do
		if not c.nested and not NOT_SERVERS[c.program or ""] then
			last = c
		end
	end
	return last, false
end

-- Whether the command is work that ends on its own, a build or a test suite,
-- rather than a server or a watcher.
local function one_off(argv: { string }): boolean
	local program = jouet.basename(argv[1] or "")
	local sub = argv[2] or ""
	if program == "cargo" then
		return sub ~= "watch" and sub ~= "leptos"
	end
	if program == "go" or program == "make" or program == "pytest" or program == "jest" then
		return true
	end
	if program == "vitest" then
		return sub == "run"
	end
	if PACKAGE_MANAGERS[program] or program == "mise" then
		local script = if sub == "run" then argv[3] or "" else sub
		return string.match(script, "^test") ~= nil
			or string.match(script, "^build") ~= nil
			or script == "ci"
	end
	return false
end

local rule: JouetRule = {
	check = function(call: JouetCall): JouetDecision?
		local c, ampersand = server(call.commands)
		if not c then
			return nil
		end
		local name, kind = name_of(c.argv)
		local command = shell_words(c.argv)
		if one_off(c.argv) then
			-- In the harness's own background, the harness says when it ends.
			if not ampersand then
				return nil
			end
			return jouet.deny(
				`Run \`jouet job start {name} -- {command}\` instead, then \`jouet wait job {name}\`: it blocks until the job ends and shows the end of its log if it fails, where a background job leaves you polling it. \`jouet job logs {name}\` reads its output meanwhile.`
			)
		end
		local url = polled_url(call.commands)
		local port = given_port(c.argv) or DEFAULT_PORTS[kind]
		local ready = if url
			then ` --http {quote(url)}`
			elseif port then ` --port {port}`
			else ""
		local run = `jouet job start {name}{ready} -- {command}`
		local after = `Then \`jouet job logs {name} -n 50\` shows its output and \`jouet job stop {name}\` stops it.`
		local message = if ready ~= ""
			then `Run \`{run}\` instead. It returns once the server answers, or fails with the end of its log, where a background job returns at once and leaves you polling it. {after}`
			else `Run \`{run}\` instead, with a ready check: \`--port N\`, \`--http URL\` or \`--output REGEX\` (a line it prints once it is up), so it returns once the server is ready, or fails with the end of its log. {after}`
		if ampersand then
			return jouet.deny(message)
		end
		return jouet.warn(message)
	end,
}

return rule

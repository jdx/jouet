--!strict
-- The `unbounded-query` rule's script (unbounded-query.toml). Its matchers
-- found `sqlite3 DB SQL` or `psql -c SQL` printing a SELECT, or `sqlite3 DB
-- .dump`, to the transcript. This lets through a query that bounds itself:
-- one with a LIMIT, or an aggregate with no GROUP BY, which is one row.

-- sqlite3's options that take a value, so the database is the first word
-- that is neither an option nor one's value.
local SQLITE_VALUES = {
	["-cmd"] = true, ["-separator"] = true, ["-newline"] = true, ["-nullvalue"] = true,
	["-init"] = true, ["-vfs"] = true, ["-maxsize"] = true, ["-mmap"] = true,
	["-lookaside"] = true, ["-pagecache"] = true, ["-heap"] = true,
}

local function query(sql: string): boolean
	local s = string.lower(sql)
	return string.match(s, "^%s*select%f[%W]") ~= nil
		or string.match(s, "^%s*with%f[%W]") ~= nil
		or string.match(s, "^%s*table%f[%W]") ~= nil
end

-- Whether the statement bounds its own output.
local function bounded(sql: string): boolean
	local s = string.lower(sql)
	if string.find(s, "%f[%w]limit%f[%W]") or string.find(s, "%f[%w]fetch%s+first%f[%W]") then
		return true
	end
	if string.find(s, "%f[%w]group%s+by%f[%W]") then
		return false
	end
	for _, f in { "count", "sum", "min", "max", "avg", "total", "group_concat", "string_agg" } do
		if string.find(s, "%f[%w]" .. f .. "%s*%(") then
			return true
		end
	end
	return false
end

-- `'sql'`, quoted for a POSIX shell.
local function quote(s: string): string
	return "'" .. string.gsub(s, "'", "'\\''") .. "'"
end

-- The database and the statement, or nil when this is not a query to
-- warn about.
local function sqlite(c: JouetCommand): (string?, string?)
	local db: string? = nil
	local i = 2
	while i <= #c.argv do
		local word = c.argv[i]
		local flag = string.gsub(word, "^%-%-", "-")
		if SQLITE_VALUES[flag] then
			i += 1
		elseif string.sub(word, 1, 1) ~= "-" then
			if db == nil then
				db = word
			elseif word == ".dump" or query(word) then
				return db, word
			end
		end
		i += 1
	end
	return nil, nil
end

local function psql(c: JouetCommand): string?
	for i = 2, #c.argv do
		local word = c.argv[i]
		local value = string.match(word, "^%-%-command=(.*)$")
		if not value and (word == "-c" or word == "--command") then
			value = c.argv[i + 1]
		end
		if value and query(value) then
			return value
		end
	end
	return nil
end

local rule: JouetRule = {
	check = function(call: JouetCall): JouetDecision?
		for _, c in call.commands do
			local db, sql = nil, nil
			if c.program == "sqlite3" then
				db, sql = sqlite(c)
			elseif c.program == "psql" then
				sql = psql(c)
			end
			if sql and (sql == ".dump" or not bounded(sql)) then
				local run = if sql == ".dump" then "jouet db schema" else `jouet db query {quote(sql)}`
				if db then
					run ..= ` --url {quote(db)}`
				end
				return jouet.warn(
					`Run \`{run}\` instead. It prints the first 50 rows and how many more there are, cuts long values, and refuses a statement that writes (\`jouet agent-help db\`).`
				)
			end
		end
		return nil
	end,
}

return rule

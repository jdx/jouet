#!/usr/bin/env bats

# `joue`: routines, Luau programs on jouet's primitives. HOME is a scratch
# directory, so user routines live in $HOME/.config/jouet/routines and trust
# in $XDG_STATE_HOME. GitHub is an address nothing listens on: nothing here
# reaches it.

setup() {
	load "test_helper/common_setup"
	_common_setup
	unset XDG_CONFIG_HOME XDG_CACHE_HOME
	export JOUET_GITHUB_API_URL="http://127.0.0.1:9"
	export JOUET_GITHUB_TOKEN="test-token"
	# `joue` as a link to jouet, the way argv[0] dispatch sees it.
	ln -s "$JOUET_BIN" "$BATS_TEST_TMPDIR/bin/joue"
	ROUTINES="$HOME/.config/jouet/routines"
	mkdir -p "$ROUTINES"
}

@test "--list shows the built-in routines and what they reach" {
	run --separate-stderr joue --list

	assert_success
	assert_line --regexp '^land +Land a PR: ask pr-next, wait for CI'
	assert_line --regexp '^pr-next +Say the one thing each PR needs next'
	assert_line --regexp '^ +reaches github.read, mise.run$'

	run --separate-stderr joue --list --json
	run jq -c '[.routines[].name]' <<<"$output"
	assert_output '["land","pr-next","ship-check","sweep"]'
}

@test "joue, the launcher and jouet run are the same command" {
	launcher="$(dirname "$JOUET_BIN")/joue"
	[[ -x "$launcher" ]] || skip "the joue launcher is not built"

	run --separate-stderr "$launcher" --list --json
	assert_success
	local listed="$output"
	run --separate-stderr jouet run --list --json
	assert_equal "$output" "$listed"
	run --separate-stderr joue --list --json
	assert_equal "$output" "$listed"
}

@test "a routine's --help comes from its usage spec" {
	run --separate-stderr joue pr-next --help

	assert_success
	assert_line 'Usage: joue pr-next [FLAGS] [prs]…'
	assert_output --partial '--settle <DUR>'
	assert_output --partial 'stops at the first state that applies'
}

@test "joue test runs every built-in routine's examples against recorded responses" {
	run --separate-stderr joue test

	assert_success
	assert_line 'ok   pr-next: a merged PR is done and needs nothing'
	assert_line --regexp '^4 routines, [0-9]+ examples, all passed$'
}

@test "a routine of yours parses its args, prints text and returns JSON" {
	cat >"$ROUTINES/greet.luau" <<'LUAU'
--- about: Greet someone
--- usage: arg "<who>" help="Who to greet"
--- usage: flag "--loud" help="Shout"
return {
	run = function(args)
		local text = `hello {args.who}`
		print(if args.loud then string.upper(text) else text)
		return { greeted = args.who }, if args.loud then joue.exit.act else 0
	end,
}
LUAU

	run --separate-stderr joue greet world
	assert_success
	assert_output 'hello world'

	run --separate-stderr joue greet world --loud --json
	assert_failure 10
	assert_output '{"greeted":"world"}'

	run --separate-stderr joue greet
	assert_failure 2
	[[ "$stderr" == "jouet: greet: "*"<who>"* ]]
}

@test "a primitive outside the declared capabilities is refused" {
	printf -- '--- about: t\nreturn { run = function() return git.state() end }\n' >"$ROUTINES/peek.luau"

	run --separate-stderr joue peek
	assert_failure 4
	assert_equal "${stderr_lines[0]}" "jouet: git.state needs the \`git.read\` capability, which peek does not declare"
	assert_equal "${stderr_lines[1]}" "hint: add it to the header: \`--- capabilities: git.read\`"
}

@test "a routine that writes previews without --yes, and acts with it" {
	echo "draft" >notes.txt
	cat >"$ROUTINES/finish.luau" <<'LUAU'
--- about: Mark the notes final
--- capabilities: fs.read, fs.write
return {
	run = function()
		local result = edit.apply("=== notes.txt\n<<<<<<< SEARCH\ndraft\n=======\nfinal\n>>>>>>> REPLACE\n")
		print(if result.dry_run then "previewed" else "written")
	end,
	examples = { { name = "it previews", exit = 4, output = "previewed", planned = { "edit notes.txt" } } },
}
LUAU

	run --separate-stderr joue finish
	assert_failure 4
	assert_line 'previewed'
	assert_line '  edit notes.txt'
	[[ "$stderr" == *"run it again with --yes"* ]]
	assert_content notes.txt $'draft\n'

	run --separate-stderr joue finish --dry-run
	assert_success
	assert_content notes.txt $'draft\n'

	run --separate-stderr joue test finish
	assert_success

	run --separate-stderr joue finish --yes
	assert_success
	assert_output 'written'
	assert_content notes.txt $'final\n'
}

@test "a routine that writes refuses to run without examples" {
	printf -- '--- about: t\n--- capabilities: github.write\nreturn { run = function() end }\n' >"$ROUTINES/bare.luau"

	run --separate-stderr joue bare --yes
	assert_failure 4
	[[ "$stderr" == *"bare writes, so it needs examples that \`joue test\` can run"* ]]
}

@test "repository routines run only once trusted" {
	mkdir -p repo/.git repo/.jouet/routines
	cd repo
	printf -- '--- about: a team routine\nreturn { run = function() return "team ran" end }\n' >.jouet/routines/team.luau

	run --separate-stderr joue --list
	assert_line --regexp '^team +a team routine \(untrusted\)$'
	assert_line --partial 'is not trusted, so its routines do not run'
	run --separate-stderr joue team
	assert_failure 4

	run --separate-stderr joue --trust
	assert_output --partial '(1 routines)'
	run --separate-stderr joue team
	assert_success
	assert_output 'team ran'

	run --separate-stderr joue --untrust
	assert_output --partial 'no longer trusting'
}

@test "joue and jouet point at each other's commands" {
	run --separate-stderr joue pr status
	assert_failure 3
	assert_equal "${stderr_lines[0]}" "jouet: no routine \`pr\`"
	assert_equal "${stderr_lines[1]}" "hint: did you mean \`jouet pr status\`? It is one of jouet's own commands"

	run --separate-stderr jouet sweep
	assert_failure 2
	assert_equal "${stderr_lines[1]}" "hint: \`sweep\` is a routine: run it with \`joue sweep\`"

	run --separate-stderr joue swep
	assert_failure 3
	assert_equal "${stderr_lines[1]}" "hint: did you mean \`joue sweep\`?"
}

@test "GitHub out of reach is a remote error from a routine too" {
	run --separate-stderr joue pr-next 7 --repo jdx/demo --json
	assert_failure 5
	assert_equal "$(jq -r '.error.code' <<<"$output")" "remote"
}

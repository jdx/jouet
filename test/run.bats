#!/usr/bin/env bats

# `jouet run`: recipes, Luau scripts that sequence jouet's commands. HOME is a
# scratch directory, so user recipes live in $HOME/.config/jouet/recipes and
# trust in $XDG_STATE_HOME. Nothing here reaches GitHub.

setup() {
	load "test_helper/common_setup"
	_common_setup
	unset XDG_CONFIG_HOME XDG_CACHE_HOME
	RECIPES="$HOME/.config/jouet/recipes"
	mkdir -p "$RECIPES"
}

@test "--list shows the built-in recipes and what they take" {
	run --separate-stderr jouet run --list

	assert_success
	assert_line --regexp '^land +land this branch.s PR: pr next, wait for CI'
	assert_line --regexp '^ship-check +before saying done'
	assert_line --regexp '^sweep +list a PR.s unresolved review threads'
	assert_line --regexp '^ +args: \[PR\] \[yes=true\]'

	run --separate-stderr jouet run --list --json
	run jq -c '[.recipes[].name]' <<<"$output"
	assert_output '["land","ship-check","sweep"]'
}

@test "--dry-run prints each step and runs none" {
	run --separate-stderr jouet run --dry-run land 123 yes=true

	assert_success
	assert_output "land would run:
  1. jouet pr next 123
  2. jouet wait ci 123
  3. jouet pr next 123
  4. jouet pr land 123 --yes"
	assert_equal "$stderr" ""

	# Without yes=true, the last step only shows the plan.
	run --separate-stderr jouet run --dry-run land --json
	run jq -c '[.dry_run, .steps[-1].command]' <<<"$output"
	assert_output '[true,"jouet pr land --dry-run"]'
}

@test "a recipe runs jouet's commands and reads its probes" {
	git init -q -b main .
	printf 'test:\n\t@echo all good\n' >Makefile
	touch ready
	cat >"$RECIPES/check.luau" <<'LUAU'
--- why: a fixture recipe
return {
	usage = "[NAME]",
	run = function(args)
		local ready = jouet.cmd.wait_file({ path = "ready", timeout = "5s", quiet = true })
		local test = jouet.cmd.test()
		local state = jouet.git.state()
		return {
			ok = ready.ok and test.ok,
			message = `hello {args[1]}: {ready.json.result} {test.command} {test.json.tail[1]} on {state.branch}`,
		}
	end,
}
LUAU

	run --separate-stderr jouet run check you

	assert_success
	assert_output 'hello you: met make test all good on main'
	[[ "$stderr" == *"jouet run: jouet wait file ready --timeout=5s --quiet"* ]]
	[[ "$stderr" == *"jouet run: make test"* ]]

	run --separate-stderr jouet run check --json
	run jq -c '[.ok, [.steps[].cmd], .steps[1].exit_code]' <<<"$output"
	assert_output '[true,["wait_file","test"],0]'
}

@test "a command that merges or posts needs yes=true, and arbitrary commands do not exist" {
	printf -- '--- why: t\nreturn { run = function() return jouet.cmd.pr_land({ pr = "1" }) end }\n' >"$RECIPES/merge.luau"
	printf -- '--- why: t\nreturn { run = function() return jouet.cmd.wait_cmd({ "rm" }) end }\n' >"$RECIPES/shell.luau"

	run --separate-stderr jouet run merge
	assert_failure 4
	[[ "$stderr" == "jouet: recipe merge: jouet.cmd.pr_land: this command posts or merges; pass "* ]]
	[[ "$stderr" == *"yes = true"*"dry_run = true"* ]]

	run --separate-stderr jouet run shell
	assert_failure 1
	[[ "$stderr" == *"attempt to call a nil value"* ]]
}

@test "repository recipes run only once trusted" {
	mkdir -p repo/.git repo/.jouet/recipes
	cd repo
	printf -- '--- why: a team recipe\nreturn { run = function() return "team ran" end }\n' >.jouet/recipes/team.luau

	run --separate-stderr jouet run --list
	assert_line --regexp '^team +a team recipe \(untrusted\)$'
	assert_line --partial 'is not trusted, so its recipes do not run'
	run --separate-stderr jouet run team
	assert_failure 4

	run --separate-stderr jouet run --trust
	assert_output --partial '(1 recipes)'
	run --separate-stderr jouet run team
	assert_success
	assert_output 'team ran'

	run --separate-stderr jouet run --untrust
	assert_output --partial 'no longer trusting'
}

@test "an unknown recipe is not found" {
	run --separate-stderr jouet run nope
	assert_failure 3
	[[ "$stderr" == "jouet: no recipe "*"nope"* ]]
}

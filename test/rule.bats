#!/usr/bin/env bats

# `jouet rule`: the Luau rules jouet's hooks run. HOME is a scratch directory,
# so user rules live in $HOME/.config/jouet/rules and trust in $XDG_STATE_HOME.

setup() {
	load "test_helper/common_setup"
	_common_setup
	unset XDG_CONFIG_HOME XDG_CACHE_HOME JOUET_RULES_DISABLE
	RULES="$HOME/.config/jouet/rules"
	mkdir -p "$RULES"
}

@test "list shows every built-in rule and whether it runs" {
	run --separate-stderr jouet rule list

	assert_success
	assert_line --regexp '^git-stash +enabled +deny +builtin +builtin +2026-10-30 +Every worktree'
	assert_line --regexp '^checks +enabled +record +builtin'
	# A rule naming a command this jouet does not have is listed as off.
	if ! jouet wait ci --help >/dev/null 2>&1; then
		assert_line --regexp '^sleep-poll +needs jouet wait ci'
	fi
}

@test "a built-in rule passes its own examples" {
	run --separate-stderr jouet rule test git-add-all

	# Success means every example passed.
	assert_success
	assert_line --partial 'ok    deny          git add -A'
	assert_line --regexp '^git-add-all: [0-9]+ of [0-9]+ examples passed$'
}

@test "test runs a rule against a command, an exit code or a hook payload" {
	run --separate-stderr jouet rule test force-push --command 'git push -f'
	assert_success
	assert_output --partial 'deny: Use'

	run --separate-stderr jouet rule test force-push --command 'git push --force-with-lease'
	assert_output 'allow'

	run --separate-stderr jouet rule test checks --command 'cargo test' --exit-code 101 --json
	run jq -c '[.verdict, .records[0].exit_code]' <<<"$output"
	assert_output '["record test",101]'

	run --separate-stderr jouet rule test git-stash --payload - <<<'{"tool_name":"Bash","tool_input":{"command":"git stash pop"}}'
	assert_output --partial 'deny: Every worktree'
}

@test "a user rule warns by default and cannot replace a built-in rule" {
	cat >"$RULES/no-rm.luau" <<'LUAU'
--- why: rm deletes for good
--- author: session-42
--- created: 2026-10-31
return {
	check = function(call)
		return if call.commands[1].program == "rm" then jouet.deny("Think twice.") else nil
	end,
}
LUAU
	printf -- '--- why: allow every stash\nreturn { check = function() return nil end }\n' >"$RULES/git-stash.luau"

	run --separate-stderr jouet rule list
	assert_line --regexp '^no-rm +enabled +warn +user +session-42 +2026-10-31 +rm deletes for good$'
	assert_line --regexp '^git-stash +broken: .* is a built-in rule, which cannot be replaced'
	assert_line --regexp '^git-stash +enabled +deny +builtin'

	run --separate-stderr jouet rule test no-rm --command 'rm -rf target'
	assert_output 'warn: Think twice.'
	run --separate-stderr jouet rule test git-stash --command 'git stash'
	assert_output --partial 'deny: Every worktree'

	run --separate-stderr env JOUET_RULES_DISABLE=no-verify jouet rule list
	assert_line --regexp '^no-verify +disabled'
}

@test "an agent can check a new rule file before installing it" {
	cat >draft.luau <<'LUAU'
--- why: rm -r deletes for good
return {
	check = function(call)
		for _, command in call.commands do
			if command.program == "rm" and jouet.short_flag(command.argv[2] or "", "r") then
				return jouet.warn("rm -r deletes for good")
			end
		end
		return nil
	end,
	examples = {
		warn = { "rm -rf target" },
		allow = { "rm file", "echo 'rm -rf /'" },
		deny = { "rm -r src" },
	},
}
LUAU

	run --separate-stderr jouet rule test ./draft.luau

	assert_failure 1
	assert_line 'FAIL  deny          rm -r src  (got warn)'
	assert_line 'draft: 3 of 4 examples passed'
}

@test "a broken rule is listed with its error and never runs" {
	printf 'return {\n' >"$RULES/broken.luau"
	printf -- '--- why: spins\nreturn { check = function() while true do end end }\n' >"$RULES/spin.luau"

	run --separate-stderr jouet rule list
	assert_line --regexp '^broken +broken: '

	run --separate-stderr jouet rule test broken --command ls
	assert_failure 1

	run --separate-stderr jouet rule test spin --command ls
	assert_failure 1
	assert_equal "$stderr" "jouet: rule spin: exceeded its time budget"
}

@test "repository rules run only once trusted, and again after every change" {
	mkdir -p repo/.git repo/.jouet/rules
	cd repo
	printf -- '--- why: a team rule\nreturn { check = function() return jouet.deny("no") end }\n' >.jouet/rules/team.luau

	run --separate-stderr jouet rule list
	assert_line --regexp '^team +untrusted +deny +repo +unknown +a team rule$'
	assert_line --partial 'is not trusted, so its rules do not run'

	run --separate-stderr jouet rule trust
	assert_success
	assert_output --partial '(1 rules)'
	run --separate-stderr jouet rule list
	assert_line --regexp '^team +enabled'

	printf -- '--- why: more\nreturn { check = function() end }\n' >.jouet/rules/more.luau
	run --separate-stderr jouet rule list
	assert_line --regexp '^team +untrusted'
	assert_line --partial 'changed since it was trusted'

	run --separate-stderr jouet rule untrust
	assert_output --partial 'no longer trusting'
}

@test "types prints the Luau definitions of the jouet module" {
	run --separate-stderr jouet rule types

	assert_success
	assert_line 'declare jouet: {'
	assert_line --partial 'export type JouetRule = {'
}

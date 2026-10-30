#!/usr/bin/env bats

# `jouet share`, `jouet rule gap` and `jouet skill --contribute`, offline:
# nothing here reaches GitHub (tests/share.rs posts to a fake server).

setup() {
	load "test_helper/common_setup"
	_common_setup
	unset XDG_CONFIG_HOME XDG_CACHE_HOME JOUET_WIND
	# Any request would go nowhere.
	export JOUET_GITHUB_API_URL=http://127.0.0.1:9
	RULES="$HOME/.config/jouet/rules"
	mkdir -p "$RULES" "$XDG_STATE_HOME/jouet/ledger"
	cp "$PROJECT_ROOT/test/fixtures/ledger.jsonl" "$XDG_STATE_HOME/jouet/ledger/ledger.jsonl"
}

@test "the contributor skill says a person approves every post" {
	run --separate-stderr jouet skill --contribute
	assert_success
	assert_line 'name: jouet-contribute'
	assert_output --partial 'only after the user says yes'

	run --separate-stderr jouet skill --contribute --format agents-md
	assert_line --index 0 '## Contributing to jouet'
}

@test "a rule that has not proven itself is refused, with every reason" {
	cat >"$RULES/no-tags.luau" <<'LUAU'
--- why: tags are cut by the release job
return { check = function(c) return if c.commands[1].argv[2] == "tag" then jouet.deny("no") else nil end }
LUAU
	run --separate-stderr jouet share no-tags --yes
	assert_failure 4
	[[ "$stderr" == *"is not ready to share:"* ]]
	[[ "$stderr" == *"it has no examples"* ]]
	[[ "$stderr" == *"it is in warn mode"* ]]

	run --separate-stderr jouet share
	assert_failure 2
}

@test "a missing primitive and a written wish become ideas, shown before posting" {
	cat >"$RULES/wants-ci.luau" <<'LUAU'
--- why: pushing while CI is red
return { check = function(c) return if jouet.ci_state then jouet.deny("red") else nil end }
LUAU
	run --separate-stderr jouet rule test wants-ci --command 'git push'
	assert_success

	run --separate-stderr jouet rule gap 'I kept checking the last test run; there is no primitive for it'
	assert_success

	run --separate-stderr jouet share --idea
	assert_success
	assert_line --regexp '^title: Idea: a .jouet.ci_state. primitive$'
	assert_output --partial 'I kept trying to catch: pushing while CI is red'
	assert_output --partial 'Nothing was posted.'

	run --separate-stderr jouet share --idea nope
	assert_failure 4
	[[ "$stderr" == *"recorded: ci_state, note-i-kept-checking-the-last"* ]]
}

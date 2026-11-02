#!/usr/bin/env bats

# `jouet skill` as an agent runs it.

setup() {
	load "test_helper/common_setup"
	_common_setup
}

@test "the onboarding skill prints as SKILL.md, JSON or an AGENTS.md snippet" {
	run --separate-stderr jouet skill --init
	assert_success
	assert_line 'name: jouet-init'
	refute_output --partial 'name: jouet-contribute'

	run --separate-stderr jouet skill --init --format codex --json
	assert_success
	run jq -r '.text | split("\n")[1]' <<<"$output"
	assert_success
	assert_output 'name: jouet-init'

	run --separate-stderr jouet skill --init --format agents-md
	assert_success
	assert_line --index 0 '## Initialize jouet for this project'
	refute_output --partial 'name: jouet-init'

	run --separate-stderr jouet skill --init --contribute
	assert_failure 2
}

@test "skill prints a thin SKILL.md, or an AGENTS.md snippet" {
	run --separate-stderr jouet skill
	assert_success
	assert_line 'name: jouet'
	assert_output --partial 'jouet agent-help'

	run --separate-stderr jouet skill --format agents-md
	assert_success
	assert_line --index 0 '## jouet'
}

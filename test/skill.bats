#!/usr/bin/env bats

# `jouet skill` as an agent runs it.

setup() {
	load "test_helper/common_setup"
	_common_setup
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

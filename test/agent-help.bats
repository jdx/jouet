#!/usr/bin/env bats

# `jouet agent-help` as an agent runs it.

setup() {
	load "test_helper/common_setup"
	_common_setup
}

@test "agent-help covers the tool in under 80 lines and mentions installed companions" {
	printf '#!/bin/sh\n' >"$BATS_TEST_TMPDIR/bin/bd"
	chmod +x "$BATS_TEST_TMPDIR/bin/bd"

	run --separate-stderr jouet agent-help

	assert_success
	# The cap is a budget to defend, not a ceiling to fill.
	assert [ "${#lines[@]}" -le 80 ]
	assert_output --partial 'jouet edit replace <PATH> --old TEXT --new TEXT'
	assert_output --partial 'Also installed: bd (beads)'
}

@test "agent-help for an unknown command exits not found" {
	run --separate-stderr jouet agent-help frobnicate
	assert_failure 3
}

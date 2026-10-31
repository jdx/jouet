#!/usr/bin/env bats

# `jouet agent-help` as an agent runs it.

setup() {
	load "test_helper/common_setup"
	_common_setup
}

@test "agent-help covers the tool in under 40 lines and mentions installed companions" {
	printf '#!/bin/sh\n' >"$BATS_TEST_TMPDIR/bin/bd"
	chmod +x "$BATS_TEST_TMPDIR/bin/bd"

	# Only the companion this test installs, whatever else this machine has.
	PATH="$BATS_TEST_TMPDIR/bin:/usr/bin:/bin" run --separate-stderr jouet agent-help

	assert_success
	# The cap is a budget to defend, not a ceiling to fill.
	assert [ "${#lines[@]}" -le 40 ]
	assert_output --partial 'jouet edit replace <PATH> --old TEXT --new TEXT'
	assert_output --partial 'Also installed: bd (beads)'
}

@test "agent-help for an unknown command exits not found" {
	run --separate-stderr jouet agent-help frobnicate
	assert_failure 3
}

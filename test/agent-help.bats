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
	# The cap is a budget to defend, not a ceiling to fill. It counts jouet's
	# own lines: which companions are on this machine's PATH varies.
	local own
	own=$(grep -cv '^Also installed:' <<<"$output")
	assert [ "$own" -le 40 ]
	assert_output --partial 'jouet edit replace <PATHS>... --old TEXT --new TEXT'
	assert_output --partial 'Also installed: bd (beads)'
}

@test "agent-help for an unknown command exits not found" {
	run --separate-stderr jouet agent-help frobnicate
	assert_failure 3
}

@test "agent-help COMMAND prints its usage, and --full the whole guide" {
	run --separate-stderr jouet agent-help edit
	assert_success
	assert_output --partial 'usage:'
	assert_output --partial 'jouet edit replace <PATHS>... --old TEXT --new TEXT  # '
	assert_output --partial "more: \`jouet agent-help edit --full\` ("
	refute_output --partial '--expect-hash PATH=SHA256  '
	assert [ "${#output}" -le 1600 ]

	run --separate-stderr jouet agent-help edit --full
	assert_success
	assert_output --partial '--- details ---'
	assert_output --partial '--expect-hash PATH=SHA256  '
	refute_output --partial "more: \`jouet agent-help edit --full\`"

	# One document with both.
	run --separate-stderr jouet agent-help edit --json
	assert_success
	assert [ "$(jq -r '.usage | contains("agent-help edit --full")' <<<"$output")" = true ]
	assert [ "$(jq -r '.text | contains("--- details ---")' <<<"$output")" = true ]
}

@test "agent-help for a subcommand prints its whole guide" {
	run --separate-stderr jouet agent-help pr threads
	assert_success
	assert_output --partial 'jouet pr threads reply <ID>'
	run --separate-stderr jouet agent-help pr threads --json
	assert_success
	assert [ "$(jq -r 'has("usage")' <<<"$output")" = false ]
}

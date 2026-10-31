#!/usr/bin/env bats

# `jouet pr next` as an agent runs it. These need no network: GitHub is an
# address nothing listens on, so a request that is made fails as remote.

setup() {
	load "test_helper/common_setup"
	_common_setup
	export JOUET_GITHUB_API_URL="http://127.0.0.1:9"
	export JOUET_GITHUB_TOKEN="test-token"
}

@test "the help names the order and the exit codes" {
	run --separate-stderr jouet pr next --help

	assert_success
	assert_output --partial "stops at the first state that applies"
	assert_output --partial "ready_to_merge"
	assert_output --partial "the shared table on purpose"
}

@test "a bad --settle is a usage error, as one JSON document" {
	run --separate-stderr jouet pr next 7 --repo jdx/demo --settle soon --json

	assert_failure 2
	assert_equal "$(jq -r '.error.code' <<<"$output")" "usage"
	assert_equal "$(jq -r '.error.message' <<<"$output")" "--settle: \`soon\` is not a duration; write it like 90s, 10m or 1h30m"
}

@test "a bare number outside a checkout needs --repo" {
	run --separate-stderr jouet pr next 7

	assert_failure 2
	assert_equal "$stderr" "jouet: no --repo given and no GitHub remote here, so \`7\` names no repository"
}

@test "GitHub out of reach is remote, never act or wait" {
	run --separate-stderr jouet pr next 7 --repo jdx/demo --json

	assert_failure 5
	assert_equal "$(jq -r '.error.code' <<<"$output")" "remote"
}

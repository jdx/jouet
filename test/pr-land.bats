#!/usr/bin/env bats

# `jouet pr land` asks before merging only when a person is at a terminal,
# and refuses everyone else without --yes before contacting GitHub. GitHub
# is an address nothing listens on, so a request that is made fails as
# remote (exit 5); a usage error (exit 2) shows none was.

setup() {
	load "test_helper/common_setup"
	_common_setup
	export JOUET_GITHUB_API_URL="http://127.0.0.1:9"
	export JOUET_GITHUB_TOKEN="test-token"
}

refusal="jouet: pr land merges the PR, and with no person at a terminal to confirm it needs --yes"

@test "without a terminal, --yes is required" {
	run --separate-stderr jouet pr land 7 --repo jdx/demo

	assert_failure 2
	assert_equal "$stderr" "$refusal
hint: pass --yes to merge without asking, or --dry-run to see the plan"
}

@test "an agent at a terminal is not asked either" {
	run in_tty CLAUDECODE=1 jouet pr land 7 --repo jdx/demo

	assert_failure 2
	assert_output --partial "$refusal"
}

@test "--json is never asked" {
	run in_tty jouet pr land 7 --repo jdx/demo --json

	assert_failure 2
	assert_output --partial '"code": "usage"'
	assert_output --partial "cannot ask with --json"
}

@test "a person at a terminal gets as far as GitHub" {
	run in_tty JOUET_GITHUB_API_URL="$JOUET_GITHUB_API_URL" JOUET_GITHUB_TOKEN=test-token \
		jouet pr land 7 --repo jdx/demo --settle 0

	assert_failure 5
	refute_output --partial "$refusal"
}

@test "--yes and --dry-run go to GitHub without asking" {
	run --separate-stderr jouet pr land 7 --repo jdx/demo --yes --settle 0
	assert_failure 5

	run --separate-stderr jouet pr land 7 --repo jdx/demo --dry-run --settle 0
	assert_failure 5
}

@test "an unknown merge method is a usage error" {
	run --separate-stderr jouet pr land 7 --repo jdx/demo --yes --method fast-forward --json

	assert_failure 2
	assert_equal "$(jq -r '.error.message' <<<"$output")" "--method must be squash, merge or rebase, not \`fast-forward\`"
}

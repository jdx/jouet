#!/usr/bin/env bats

setup() {
	load "test_helper/common_setup"
	_common_setup
	unset XDG_CONFIG_HOME XDG_CACHE_HOME
	init_repo
	mkdir -p "$BATS_TEST_TMPDIR/tools"
	printf '#!/bin/sh\nexit 0\n' >"$BATS_TEST_TMPDIR/tools/cargo"
	chmod +x "$BATS_TEST_TMPDIR/tools/cargo"
	export PATH="$BATS_TEST_TMPDIR/tools:$PATH"
	export JOUET_COST_RECORD=0 JOUET_TEST_BASELINE=off JOUET_SESSION_ID=private-session
}

@test "uses the Codex thread ID when no jouet session ID is set" {
	unset JOUET_SESSION_ID CLAUDE_CODE_SESSION_ID
	export CODEX_THREAD_ID=private-codex-thread
	for _ in 1 2 3; do
		run --separate-stderr env JOUET_COST_SEQUENCES=1 jouet test --cmd -- cargo clippy
		assert_success
		run --separate-stderr env JOUET_COST_SEQUENCES=1 jouet test --cmd -- cargo test
		assert_success
	done
	run --separate-stderr jouet cost --sequences --json
	assert_success
	run jq -c '[.sequences[] | select(.before == "cargo clippy" and .after == "cargo test") | .samples]' <<<"$output"
	assert_output '[3]'
	run grep -r 'private-codex-thread' "$XDG_STATE_HOME/jouet/cost-sequences"
	assert_failure 1
}

@test "records successive checks only when opted in and reports local observations" {
	run --separate-stderr env JOUET_COST_SEQUENCES=0 jouet test --cmd -- cargo test
	assert_success
	[[ ! -d "$XDG_STATE_HOME/jouet/cost-sequences" ]]

	unset JOUET_SESSION_ID CODEX_THREAD_ID CLAUDE_CODE_SESSION_ID
	run --separate-stderr env JOUET_COST_SEQUENCES=1 jouet test --cmd -- cargo test
	assert_success
	[[ ! -d "$XDG_STATE_HOME/jouet/cost-sequences" ]]
	export JOUET_SESSION_ID=private-session
	for _ in 1 2 3; do
		run --separate-stderr env JOUET_COST_SEQUENCES=1 jouet test --cmd -- cargo clippy
		assert_success
		run --separate-stderr env JOUET_COST_SEQUENCES=1 jouet test --cmd -- cargo test
		assert_success
	done
	run --separate-stderr jouet cost --sequences --json
	assert_success
	run jq -c '[.sequences[] | select(.before == "cargo clippy" and .after == "cargo test") | .samples]' <<<"$output"
	assert_output '[3]'
	run --separate-stderr jouet cost --sequences
	assert_success
	assert_line --partial 'other work and cache changes may intervene'
	assert_line --partial 'cargo clippy → cargo test'
	run grep -r 'private-session' "$XDG_STATE_HOME/jouet/cost-sequences"
	assert_failure 1
}

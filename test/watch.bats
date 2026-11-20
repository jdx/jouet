#!/usr/bin/env bats

# `jouet watch`: the PR watcher Claude Code's background hook starts after a
# push. These are the calls that end at once; the wait itself, against a fake
# GitHub, is in tests/wait_github.rs.

setup() {
	load 'test_helper/common_setup'
	_common_setup
	export XDG_CONFIG_HOME="$BATS_TEST_TMPDIR/config"
	unset JOUET_HOOKS JOUET_HOOKS_LEDGER CLAUDE_PID
	export AI_AGENT=claude-code_2-1-318_harness
	ledger="$XDG_STATE_HOME/jouet/ledger/ledger.jsonl"
}

# The JSON Claude Code gives a PostToolUse hook for the Bash COMMAND of call ID.
payload() {
	jq -cn --arg id "$1" --arg command "$2" --arg cwd "$PWD" \
		'{session_id: "s1", hook_event_name: "PostToolUse", tool_name: "Bash", tool_use_id: $id,
		  cwd: $cwd, tool_input: {command: $command}, tool_response: {}}'
}

# Runs a command as a session in the settle arm.
in_arm() {
	JOUET_EXPERIMENT_SETTLE_SHARE=100 "$@"
}

# What the ledger says became of each push.
watched() {
	[[ -f "$ledger" ]] || return 0
	jq -r 'select(.event == "watch") | .settle.result' "$ledger"
}

@test "watch says nothing and exits 0 outside the settle arm, which is the default" {
	run --separate-stderr jouet watch claude <<<"$(payload t1 'git push')"

	assert_success
	assert_output ''
	assert_equal "$stderr" ''
	assert [ ! -e "$XDG_STATE_HOME/jouet/watch" ]
	run watched
	assert_output ''
}

@test "watch exits 0 at once for a call that is not a push that worked" {
	for command in 'git status' 'echo pushed' 'gh pr view 12'; do
		run --separate-stderr in_arm jouet watch claude <<<"$(payload t1 "$command")"
		assert_success
		assert_output ''
		assert_equal "$stderr" ''
	done
	# A push that failed is another event, which the entry is not installed for.
	run --separate-stderr in_arm jouet watch claude <<<"$(payload t2 'git push' | jq -c '.hook_event_name = "PostToolUseFailure"')"
	assert_success
	assert_equal "$output$stderr" ''
	run watched
	assert_output ''
}

@test "watch fails open: stdin it cannot read, another harness and a bad flag all exit 0" {
	run --separate-stderr in_arm jouet watch claude <<<'{ not json'
	assert_success
	assert_equal "$output$stderr" ''
	run --separate-stderr in_arm jouet watch claude </dev/null
	assert_success
	assert_equal "$output$stderr" ''
	run --separate-stderr in_arm jouet watch codex <<<"$(payload t1 'git push')"
	assert_success
	assert_equal "$output$stderr" ''
	run --separate-stderr in_arm jouet watch <<<"$(payload t2 'git push')"
	assert_success
	assert_equal "$output$stderr" ''
	run --separate-stderr in_arm jouet watch claude --timeout never <<<"$(payload t3 'git push')"
	assert_success
	assert_equal "$output$stderr" ''
	run watched
	assert_output ''
}

@test "a push gets no watcher where the hook is not known to run in the background, and the ledger says so" {
	# No CLAUDE_PID: nothing says how Claude Code was started.
	run --separate-stderr in_arm jouet watch claude <<<"$(payload t1 'git push -u origin feat')"
	assert_success
	assert_equal "$output$stderr" ''
	# A Claude Code older than the one the wake was tried with.
	AI_AGENT=claude-code_2-1-200_harness CLAUDE_PID=$$ \
		run --separate-stderr in_arm jouet watch claude <<<"$(payload t2 'gh pr create --fill')"
	assert_success
	assert_equal "$output$stderr" ''
	# The second hook entry a compound command starts does nothing more.
	run --separate-stderr in_arm jouet watch claude <<<"$(payload t2 'gh pr create --fill')"
	assert_success

	run watched
	assert_output 'foreground
foreground'
	assert [ ! -e "$XDG_STATE_HOME/jouet/watch" ]
}

@test "watch is a hook entry, not a command jouet lists" {
	run --separate-stderr jouet --help
	assert_success
	refute_line --regexp '^ *watch( |$)'
	run --separate-stderr jouet agent-help
	assert_success
	refute_output --partial 'jouet watch'
}

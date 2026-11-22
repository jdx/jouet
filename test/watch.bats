#!/usr/bin/env bats
# shellcheck disable=SC2016 # a `$VAR` in single quotes is the agent's command, as the hook is given it

# `jouet watch`: the PR watcher Claude Code's background hook starts after a
# push. These are the calls that end at once; the wait itself, against a fake
# GitHub, is in tests/wait_github.rs.

setup() {
	load 'test_helper/common_setup'
	_common_setup
	export XDG_CONFIG_HOME="$BATS_TEST_TMPDIR/config"
	unset JOUET_HOOKS JOUET_HOOKS_LEDGER JOUET_EXPERIMENT_SETTLE_SHARE CLAUDE_PID
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

# The work tree each push was looked up in, as the ledger's hash of it.
watched_in() {
	jq -r 'select(.event == "watch") | .repo // "none"' "$ledger"
}

# The session's own checkout, where the test runs, and another one under the
# home directory. Sets `here` and `there` to what the ledger calls them.
two_checkouts() {
	git init -q
	git init -q "$HOME/src/other"
	mkdir -p "$HOME/src/other/docs"
	in_arm jouet watch claude <<<"$(payload here 'git push')"
	(cd "$HOME/src/other" && in_arm jouet watch claude <<<"$(payload there 'git push')")
	here=$(watched_in | sed -n 1p)
	there=$(watched_in | sed -n 2p)
	assert [ -n "$here" ] && assert [ -n "$there" ]
	assert [ "$here" != none ] && assert [ "$there" != none ]
	assert [ "$here" != "$there" ]
}

# Runs the hook for each COMMAND as a call in the session's own directory and
# expects the ledger to record it for the work tree REPO.
recorded_for() {
	local repo=$1 command said n=0
	shift
	for command in "$@"; do
		n=$((n + 1))
		said=$(in_arm jouet watch claude <<<"$(payload "$repo-$n" "$command")" 2>&1)
		assert_equal "$said" ''
		assert_equal "$(watched_in | tail -n 1) after: $command" "$repo after: $command"
	done
	assert_equal "$(watched_in | grep -c '')" "$((n + 2))"
}

@test "a push is looked up in the checkout it ran in, which a cd or git -C before it names" {
	two_checkouts
	# The first three are pushes the watcher looked for in the session's
	# directory on its first day.
	recorded_for "$there" \
		'cd ~/src/other && git add tasks.md && git commit -q -m "docs(tasks): say what lint runs" && git push 2>&1 | tail -2 && jouet pr threads reply PRRT_1 --body done' \
		'cd ~/src/other && git push -u origin claude/fix-silent-otlp-export' \
		'cd ~/src/other && gh pr create --base main --head claude/fix-silent-otlp-export --fill' \
		'cd $HOME/src/other; git add a.rs; git commit -q -m m; git push' \
		'cd ../home/src/other/docs && gh stack submit' \
		"cd '$HOME/src/other' || exit 1
git push" \
		'git -C ~/src/other push' \
		"git -C $HOME/src/other/docs push -u origin feat"
	run watched
	assert_equal "$(sort -u <<<"$output")" foreground
}

@test "a push whose directory the command does not settle is looked up in the session's" {
	two_checkouts
	recorded_for "$here" \
		'cd "$DIR" && git push' \
		'git -C "$DIR" push' \
		'(cd ~/src/other && git push)' \
		'cd ~/src/other | cat; git push' \
		'cd ~/src/gone && git push' \
		'cd ~ && git push' \
		'git -C ~/src/other status && git push'
}

@test "watch is a hook entry, not a command jouet lists" {
	run --separate-stderr jouet --help
	assert_success
	refute_line --regexp '^ *watch( |$)'
	run --separate-stderr jouet agent-help
	assert_success
	refute_output --partial 'jouet watch'
}

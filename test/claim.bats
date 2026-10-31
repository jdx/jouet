#!/usr/bin/env bats
# Messages name commands in backquotes, which the tests match literally.
# shellcheck disable=SC2016

# `jouet claim`: advisory claims between sessions, across two worktrees of
# one repository, and the hook rule that warns about them.

setup() {
	load "test_helper/common_setup"
	_common_setup
	unset JOUET_HOOKS JOUET_HOOKS_STEER JOUET_HOOKS_PRIME JOUET_HOOKS_VERIFY JOUET_HOOKS_LEDGER JOUET_RULES_DISABLE
	unset JOUET_SESSION_ID CLAUDE_CODE_SESSION_ID CODEX_THREAD_ID CLAUDE_PID
	export XDG_CONFIG_HOME="$BATS_TEST_TMPDIR/config"
	export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1
	export GIT_AUTHOR_NAME=t GIT_AUTHOR_EMAIL=t@t GIT_COMMITTER_NAME=t GIT_COMMITTER_EMAIL=t@t
	git init -q -b main repo
	mkdir -p repo/src/cmd repo/docs
	echo a >repo/src/cmd/a.rs
	echo b >repo/src/b.rs
	echo d >repo/docs/d.md
	git -C repo add .
	git -C repo commit -qm init
	git -C repo worktree add -q -b other ../wt
	cd repo || return 1
}

# A pre-tool-use hook call from Claude Code session $1, in directory $2, for
# tool input $3 (a JSON object) of tool $4 (Bash when omitted).
hook() {
	jq -cn --arg s "$1" --arg cwd "$2" --argjson input "$3" --arg tool "${4:-Bash}" \
		'{session_id: $s, cwd: $cwd, hook_event_name: "PreToolUse", tool_name: $tool, tool_input: $input}' |
		jouet hook claude pre-tool-use
}

@test "a claim in one worktree is seen, and refused to overlapping claims, from another" {
	run --separate-stderr env CLAUDE_CODE_SESSION_ID=s1-aaaaaaaa jouet claim src/cmd/ --note 'reworking the commands' --for 1h
	assert_success
	assert_line --index 0 --regexp '^CLAIMED src/cmd/ for claude session s1-aaaaa, until [0-9TZ:-]+ \(1h\)$'

	cd ../wt
	run --separate-stderr env CLAUDE_CODE_SESSION_ID=s2-bbbbbbbb jouet claim src/cmd/a.rs src/b.rs
	assert_failure 1
	assert_line --index 0 'REFUSED other sessions claim overlapping paths; nothing was claimed'
	assert_line --index 1 --regexp '^  src/cmd/a.rs: claimed as src/cmd/ by claude session s1-aaaaa in .*/repo \(branch main\), (59m|1h) left: reworking the commands$'

	run --separate-stderr env CLAUDE_CODE_SESSION_ID=s2-bbbbbbbb jouet claim ls --json
	assert_success
	assert_equal "$(jq -r '.[] | [.pattern, .session, .branch, .note] | join(" ")' <<<"$output")" 'src/cmd/ s1-aaaaaaaa main reworking the commands'
	assert_equal "$(jq '.[0].mine' <<<"$output")" null

	# src/b.rs was not claimed, since the call as a whole was refused.
	run --separate-stderr env CLAUDE_CODE_SESSION_ID=s2-bbbbbbbb jouet claim src/b.rs --json
	assert_success
	assert_equal "$(jq -r '.claimed[0] | .pattern, .branch' <<<"$output")" $'src/b.rs\nother'

	run --separate-stderr env CLAUDE_CODE_SESSION_ID=s1-aaaaaaaa jouet claim ls
	assert_line --regexp '^claimed as src/cmd/ by .* \(yours\)$'
	assert_line --regexp '^claimed as src/b.rs by claude session s2-bbbbb in .*/wt \(branch other\), (1h|2h) left$'
}

@test "claims lapse when they expire or their worktree goes, and release takes only your own" {
	CLAUDE_CODE_SESSION_ID=s1 jouet claim docs/ --for 1s >/dev/null
	(cd ../wt && CLAUDE_CODE_SESSION_ID=s2 jouet claim src/b.rs >/dev/null)
	sleep 1.2
	run --separate-stderr jouet claim ls --all --json
	assert_equal "$(jq -r '.[] | [.pattern, .lapsed // "live"] | join(" ")' <<<"$output")" $'docs/ expired\nsrc/b.rs live'

	git worktree remove --force ../wt
	run --separate-stderr jouet claim ls --json
	assert_success
	assert_output '[]'

	CLAUDE_CODE_SESSION_ID=s1 jouet claim src/b.rs docs/d.md >/dev/null
	run --separate-stderr env CLAUDE_CODE_SESSION_ID=s3 jouet claim release src/b.rs
	assert_failure 3
	assert_equal "$stderr" $'jouet: this session claims none of src/b.rs\nhint: `jouet claim ls` lists the claims'
	run --separate-stderr env CLAUDE_CODE_SESSION_ID=s1 jouet claim release src/b.rs
	assert_success
	assert_output 'RELEASED src/b.rs'
	run --separate-stderr env CLAUDE_CODE_SESSION_ID=s1 jouet claim release --json
	assert_equal "$(jq -r '.released[].pattern' <<<"$output")" docs/d.md
}

@test "a claim lapses when the agent process that made it exits" {
	sleep 30 &
	local agent=$!
	CLAUDE_CODE_SESSION_ID=s1 CLAUDE_PID=$agent jouet claim src/ >/dev/null
	run --separate-stderr jouet claim ls --json
	assert_equal "$(jq -r '.[0].pid' <<<"$output")" "$agent"
	kill "$agent"
	wait "$agent" 2>/dev/null || true
	run --separate-stderr jouet claim ls --all --json
	assert_equal "$(jq -r '.[0].lapsed' <<<"$output")" 'session gone'
}

@test "claims need a session and a path inside the repository" {
	run --separate-stderr jouet claim src/
	assert_failure 4
	assert_equal "$stderr" $'jouet: no agent session: claims belong to a Claude Code or Codex session\nhint: run it from the agent\'s shell, or set JOUET_SESSION_ID'
	run --separate-stderr env JOUET_SESSION_ID=me jouet claim ../outside
	assert_failure 2
	run --separate-stderr env JOUET_SESSION_ID=me jouet claim .
	assert_failure 2
	assert_equal "$stderr" 'jouet: `.` is the whole repository: claim the files or directories you work on'
	run --separate-stderr env JOUET_SESSION_ID=me jouet claim src/ --for nope
	assert_failure 2
}

@test "the hook warns another session that edits or commits a claimed path" {
	CLAUDE_CODE_SESSION_ID=s1-aaaaaaaa jouet claim src/cmd/ --note 'reworking the commands' >/dev/null
	local wt
	wt="$(cd ../wt && pwd -P)"

	# An edit in the other worktree, by another session.
	run --separate-stderr hook s2 "$wt" "$(jq -cn --arg p "$wt/src/cmd/a.rs" '{file_path: $p, old_string: "a", new_string: "x"}')" Edit
	assert_success
	assert_equal "$(jq -r .hookSpecificOutput.additionalContext <<<"$output")" \
		"Another session is working on this: src/cmd/a.rs is claimed as src/cmd/ by session s1-aaaaa in $(pwd -P) (branch main), until $(jouet claim ls --json | jq -r '.[0].expires_at'): reworking the commands. Leave it to that session or coordinate first; \`jouet claim ls\` lists the claims. [jouet rule: claimed-path]"
	assert_equal "$(jq -r .hookSpecificOutput.permissionDecision <<<"$output")" null

	# The claiming session itself is not warned.
	run --separate-stderr hook s1-aaaaaaaa "$(pwd -P)" "$(jq -cn --arg p "$PWD/src/cmd/a.rs" '{file_path: $p}')" Edit
	assert_output ''

	# A commit of what is staged, in the other worktree.
	echo x >../wt/src/cmd/a.rs
	git -C ../wt add src/cmd/a.rs
	run --separate-stderr hook s2 "$wt" '{"command": "git commit -m wip"}'
	assert_output --partial 'src/cmd/a.rs is claimed as src/cmd/ by session s1-aaaaa'
	# `-a` takes changed tracked files; nothing claimed is changed in this one.
	run --separate-stderr hook s2 "$(pwd -P)" '{"command": "git commit -am wip"}'
	assert_output ''
	run --separate-stderr hook s2 "$wt" '{"command": "jouet commit src -m wip"}'
	assert_output --partial 'is claimed as src/cmd/'
	run --separate-stderr hook s2 "$wt" '{"command": "git commit -m wip -- src/b.rs"}'
	assert_output ''
}

#!/usr/bin/env bats

# `jouet sessions`: which agent sessions work where, read from fake Claude
# Code and Codex transcripts that point into real git worktrees.

setup() {
	load "test_helper/common_setup"
	_common_setup
	init_repo
	commit_file README.md 'hi' 'init'
	git worktree add -q -b feat/a ../wt-a
	git worktree add -q -b feat/gone ../wt-gone
	git worktree remove ../wt-gone
	WORK="$BATS_TEST_TMPDIR/work"
	WT_A="$BATS_TEST_TMPDIR/wt-a"
	export CLAUDE_CONFIG_DIR="$BATS_TEST_TMPDIR/claude"
	export CODEX_HOME="$BATS_TEST_TMPDIR/codex"
}

# The `touch -t` stamp of MINUTES ago, with GNU or BSD date.
ago() {
	local t=$(($(date +%s) - $1 * 60))
	date -d "@$t" +%Y%m%d%H%M.%S 2>/dev/null || date -r "$t" +%Y%m%d%H%M.%S
}

# A Claude Code transcript for session ID last working in DIR, written
# MINUTES ago.
claude_session() {
	local file="$CLAUDE_CONFIG_DIR/projects/p/$1.jsonl"
	mkdir -p "$(dirname "$file")"
	printf '{"type":"user","cwd":"%s","gitBranch":"from-transcript","message":{"content":"do not print me"}}\n' "$2" >"$file"
	touch -t "$(ago "$3")" "$file"
}

# A Codex rollout for session ID, started in DIR, written MINUTES ago.
codex_session() {
	local file="$CODEX_HOME/sessions/2026/10/31/rollout-2026-10-31T00-00-00-$1.jsonl"
	mkdir -p "$(dirname "$file")"
	printf '{"type":"session_meta","payload":{"id":"%s","cwd":"%s","git":{"branch":"x"}}}\n' "$1" "$2" >"$file"
	touch -t "$(ago "$3")" "$file"
}

@test "two live sessions in one worktree and a live one whose worktree is gone are warned about" {
	claude_session aaaaaaaa-0001 "$WT_A" 1
	codex_session cccccccc-0003 "$WT_A/sub" 2
	claude_session bbbbbbbb-0002 "$BATS_TEST_TMPDIR/wt-gone" 3
	claude_session dddddddd-0004 "$WORK" 120
	mkdir -p "$WT_A/sub"

	run --separate-stderr jouet sessions
	assert_success
	assert_line --regexp '^ live  claude  aaaaaaaa +[0-9]+[sm] ago  .*/wt-a  \[feat/a\]$'
	assert_line --regexp '^ live  codex   cccccccc +[0-9]+[sm] ago  .*/wt-a  \[feat/a\]$'
	assert_line --regexp '^ live  claude  bbbbbbbb .*/wt-gone  \(gone\)$'
	assert_line --regexp '^warning: 2 live sessions share the work tree .*/wt-a \(claude aaaaaaaa, codex cccccccc\)'
	assert_line --regexp '^warning: claude bbbbbbbb last worked in .*/wt-gone, which no longer exists$'
	assert_line '... 1 idle session not listed (--all lists them)'
	refute_output --partial 'do not print me'
	refute_output --partial dddddddd

	run --separate-stderr jouet sessions --json
	assert_success
	assert_equal "$(jq -r '[.warnings[].code] | join(",")' <<<"$output")" 'shared_worktree,worktree_gone'
	assert_equal "$(jq -r '.warnings[0].sessions | join(",")' <<<"$output")" 'claude:aaaaaaaa-0001,codex:cccccccc-0003'
	assert_equal "$(jq -r '.idle_not_listed' <<<"$output")" 1
	assert_equal "$(jq -r '.sessions[] | select(.id == "cccccccc-0003") | .cwd' <<<"$output")" "$WT_A/sub"
}

@test "--all lists idle sessions, and --since leaves out older ones" {
	claude_session dddddddd-0004 "$WORK" 120
	claude_session eeeeeeee-0005 "$WORK" 3000

	run --separate-stderr jouet sessions --all --json
	assert_success
	assert_equal "$(jq -r '[.sessions[].id] | join(",")' <<<"$output")" 'dddddddd-0004'
	assert_equal "$(jq -r '.sessions[0] | .live // false, .branch' <<<"$output")" $'false\nmain'

	run --separate-stderr jouet sessions --all --since 3d
	assert_success
	assert_line --regexp '^ idle  claude  eeeeeeee .*\[main\]$'

	run --separate-stderr jouet sessions
	assert_success
	assert_output $'no live agent sessions on this machine\n\n... 1 idle session not listed (--all lists them)'
}

@test "a bad --since is a usage error" {
	run --separate-stderr jouet sessions --since soon
	assert_failure 2
}

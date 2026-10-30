#!/usr/bin/env bats

# `jouet handoff` records facts for the next session; `jouet pickup` prints
# them and says whether they still hold.

setup() {
	load "test_helper/common_setup"
	_common_setup
	init_repo
	commit_file a.txt $'a\n' "init"
}

@test "a handoff picked up unchanged is still accurate" {
	printf 'a2\n' >a.txt
	printf 'n\n' >notes.txt

	run --separate-stderr jouet handoff --no-pr
	assert_success
	assert_line --index 0 "# Handoff: main"
	assert_line "  - \` M\` a.txt"
	assert_line "  - notes.txt"
	assert_line "  - pr: not looked up (--no-pr)"
	assert_line --regexp "^wrote .*/\.git/jouet/handoff\.md; the next session reads it with \`jouet pickup\`$"

	run --separate-stderr jouet pickup
	assert_success
	assert_line --index 0 "# Handoff: main"
	assert_line --regexp '^Still accurate: nothing has drifted since '
}

@test "pickup lists what drifted and exits 1" {
	printf 'a2\n' >a.txt
	jouet handoff --no-pr >/dev/null
	git commit -q -m "commit a" -- a.txt
	printf 'x\n' >scratch.txt

	run --separate-stderr jouet pickup

	assert_failure 1
	assert_line --regexp '^## Drift since '
	assert_line --regexp "^- \`head\`: 1 new commit since the handoff \\([0-9a-f]{12}\\.\\.[0-9a-f]{12}\\)\$"
	assert_line "- \`uncommitted\`: a.txt no longer has changes"
	assert_line "- \`untracked\`: now untracked: scratch.txt"
}

@test "a handoff written to a file elsewhere is picked up from there" {
	jouet handoff --no-pr -o "$BATS_TEST_TMPDIR/handoff.json" >/dev/null

	run --separate-stderr jouet pickup "$BATS_TEST_TMPDIR/handoff.json" --json

	assert_success
	assert_equal "$(jq -r .accurate <<<"$output")" true
	assert_equal "$(jq -r .handoff.head_subject <<<"$output")" init
}

@test "a background process is listed with a way to attach" {
	[[ "$(uname)" == Linux ]] || skip "processes are listed on Linux only"
	sleep 30 >server.log 2>&1 &
	local pid=$!

	run --separate-stderr jouet handoff --no-pr -o -
	kill "$pid"

	assert_success
	assert_line "  - $pid \`sleep 30\`: attach with \`tail -f $(pwd -P)/server.log\`"
}

@test "pickup without a handoff is not found" {
	run --separate-stderr jouet pickup
	assert_failure 3
	# git reports the resolved path: on macOS, /tmp is /private/tmp.
	assert_equal "$stderr" "jouet: no handoff at $(pwd -P)/.git/jouet/handoff.md; \`jouet handoff\` writes one"
}

#!/usr/bin/env bats
# Messages name commands in backquotes, which the tests match literally.
# shellcheck disable=SC2016

# jouet with Entire: the earlier sessions on the branch in `jouet pickup`,
# Entire's sessions in `jouet sessions`, and where lines came from in `jouet
# read --why` and `jouet edit --why`. A fake `entire` on PATH records its
# arguments and answers with what entire 0.11.4 prints (test/fixtures/entire).
# FAKE_ENTIRE picks the repository's state: enabled (the default), disabled or
# logged-out.

setup() {
	load "test_helper/common_setup"
	_common_setup
	init_repo
	export JOUET_ENTIRE=true
	export CLAUDE_CONFIG_DIR="$BATS_TEST_TMPDIR/claude"
	export CODEX_HOME="$BATS_TEST_TMPDIR/codex"
	export FAKE_ENTIRE_FIXTURES="$PROJECT_ROOT/test/fixtures/entire"
	export FAKE_ENTIRE_LOG="$BATS_TEST_TMPDIR/entire.log"
	: >"$FAKE_ENTIRE_LOG"
	cat >"$BATS_TEST_TMPDIR/bin/entire" <<'SH'
#!/usr/bin/env bash
# Records each call, then answers from the recorded fixtures.
printf '%s\n' "$*" >>"$FAKE_ENTIRE_LOG"
fixture() { sed "s|@WORK@|$BATS_TEST_TMPDIR/work|g" "$FAKE_ENTIRE_FIXTURES/$1"; }
case "$*" in
"status --json")
	case "${FAKE_ENTIRE:-enabled}" in
	disabled) fixture status-disabled.json ;;
	*) fixture status-enabled.json ;;
	esac
	;;
"auth status --json")
	case "${FAKE_ENTIRE:-enabled}" in
	logged-out) fixture auth-logged-out.json ;;
	*) fixture auth-ok.json ;;
	esac
	;;
"session list --json") fixture session-list.json ;;
"checkpoint list --json --no-pager") fixture checkpoint-list.json ;;
"why "*" --json")
	[[ -n ${FAKE_ENTIRE_SLOW:-} ]] && exec sleep 5
	file="why-$(basename "$2").json"
	if [[ ! -f $FAKE_ENTIRE_FIXTURES/$file ]]; then
		echo "git blame --line-porcelain $2: exit status 128 (stderr: fatal: no such path '$2' in HEAD)" >&2
		exit 1
	fi
	fixture "$file"
	;;
*)
	echo "fake entire: unexpected call: $*" >&2
	exit 99
	;;
esac
SH
	chmod +x "$BATS_TEST_TMPDIR/bin/entire"
	commit_file wait_port.sh $'#!/bin/sh\nport=""\nfor _ in $(seq 1 100); do\n\tport=$(cat port 2>/dev/null || true)\ndone\necho "$port"\n' 'init'
}

# Fails if jouet ran anything but the read-only calls it is allowed.
assert_only_read_only_calls() {
	run grep -Ev '^(status --json|auth status --json|session list --json|checkpoint list --json --no-pager|why [^ ]+ --json)$' "$FAKE_ENTIRE_LOG"
	assert_failure
}

# PATH without the fake or any real `entire`.
path_without_entire() {
	local dir out=""
	local IFS=:
	for dir in $PATH; do
		[[ -x $dir/entire ]] && continue
		out="$out${out:+:}$dir"
	done
	printf '%s' "$out"
}

@test "pickup lists the earlier sessions on the branch from Entire's checkpoints, without this one" {
	jouet handoff --no-pr >/dev/null
	export CLAUDE_CODE_SESSION_ID=b58a7ecd-5735-44d9-923b-36cac0d93141

	run --separate-stderr jouet pickup
	assert_success
	assert_line '## Entire: earlier sessions on this branch'
	assert_line --regexp '^- Claude Code `8d2ac41d` \(ended, active [0-9]+[smhd] ago\): 2 checkpoints$'
	assert_line '  - last checkpoint `01M66H17P11G` at 2026-10-30T13:47:06Z: test: bound the flaky-test checks against slow or missing ports'
	assert_line '  - files: wait_port.sh, src/mcp/page.rs, test/proxy_mcp.bats, mise.lock, Cargo.lock, ... 1 more'
	assert_line --regexp '^- Codex `b7460ac0` \(ended, active [0-9]+[smhd] ago\): 1 checkpoint$'
	# Entire no longer lists this session, and its checkpoint is not committed:
	# the message is its prompt.
	assert_line '- session `e0ce30a0`: 1 checkpoint'
	assert_line '  - last checkpoint `21832ae34773` at 2026-10-28T13:00:03Z: Yes, fix both flaky tests in a separate PR off main.'
	assert_line '- ... 1 more session (`entire checkpoint list`)'
	assert_line --partial 'More: `entire session info 8d2ac41d-2f33-454f-8d12-c98a0770a25b`'
	assert_line --partial 'Still accurate'
	refute_output --partial b58a7ecd
	refute_output --partial FAKE-TOKEN-SOURCE
	assert_only_read_only_calls
	# Each question is asked once.
	run grep -c '^status --json$' "$FAKE_ENTIRE_LOG"
	assert_output 1
}

@test "pickup --json carries Entire's sessions" {
	jouet handoff --no-pr >/dev/null

	run --separate-stderr jouet pickup --json
	assert_success
	run jq -c '.entire | {checkpoints, more: .more_sessions, first: .sessions[0] | {id, agent, status, checkpoints, last: .last_checkpoint.id, file_count}}' <<<"$output"
	assert_output '{"checkpoints":6,"more":2,"first":{"id":"8d2ac41d-2f33-454f-8d12-c98a0770a25b","agent":"Claude Code","status":"ended","checkpoints":2,"last":"01M66H17P11G5CYSP65VHNQVMB","file_count":6}}'
}

@test "pickup says nothing of Entire where it is disabled, logged out or missing" {
	jouet handoff --no-pr >/dev/null

	FAKE_ENTIRE=disabled run --separate-stderr jouet pickup --json
	assert_success
	run jq 'has("entire")' <<<"$output"
	assert_output false
	# Disabled: the login is not even asked about.
	run cat "$FAKE_ENTIRE_LOG"
	assert_output 'status --json'

	: >"$FAKE_ENTIRE_LOG"
	FAKE_ENTIRE=logged-out run --separate-stderr jouet pickup
	assert_success
	refute_output --partial Entire
	assert_equal "$stderr" ""
	run cat "$FAKE_ENTIRE_LOG"
	assert_output $'status --json\nauth status --json'

	PATH="$(path_without_entire)" run --separate-stderr "$JOUET_BIN" pickup
	assert_success
	refute_output --partial Entire
	assert_equal "$stderr" ""
}

@test "JOUET_ENTIRE=false never runs entire" {
	jouet handoff --no-pr >/dev/null
	JOUET_ENTIRE=false run --separate-stderr jouet pickup
	assert_success
	refute_output --partial Entire
	run cat "$FAKE_ENTIRE_LOG"
	assert_output ''
}

@test "a slow entire is a note in pickup, not an error" {
	jouet handoff --no-pr >/dev/null
	cat >"$BATS_TEST_TMPDIR/bin/entire" <<'SH'
#!/usr/bin/env bash
case "$*" in
"status --json") echo '{"enabled":true}' ;;
"auth status --json") echo '{"logged_in":true}' ;;
*) exec sleep 5 ;;
esac
SH
	JOUET_ENTIRE_TIMEOUT_MS=300 run --separate-stderr jouet pickup
	assert_success
	assert_line '- note: `entire checkpoint list --json --no-pager` took longer than 300ms, so jouet stopped waiting'
}

@test "sessions adds Entire's sessions, once each" {
	local file="$CLAUDE_CONFIG_DIR/projects/p/8d2ac41d-2f33-454f-8d12-c98a0770a25b.jsonl"
	mkdir -p "$(dirname "$file")"
	printf '{"type":"user","cwd":"%s","gitBranch":"main","message":{"content":"x"}}\n' "$BATS_TEST_TMPDIR/work" >"$file"

	run --separate-stderr jouet sessions --all --since 2020-01-01
	assert_success
	assert_line --regexp '^ live  claude  8d2ac41d .*/work  \[main\]  \(Entire: 10 checkpoints\)$'
	assert_line --regexp '^ idle  cursor  c0ffee00 .*/work  \[main\]  \(only in Entire, 2 checkpoints\)$'
	assert_line --regexp '^ idle  codex   b7460ac0 .*  \(only in Entire, 1 checkpoint\)$'
	run grep -c 8d2ac41d <<<"$output"
	assert_output 1
	refute_output --partial FAKE-TOKEN-SOURCE

	run --separate-stderr jouet sessions --all --since 2020-01-01 --json
	run jq -c '[.sessions[] | {harness, id: .id[0:8], entire}] | sort_by(.id)' <<<"$output"
	assert_output '[{"harness":"claude","id":"8d2ac41d","entire":{"status":"ended","checkpoints":10}},{"harness":"claude","id":"b58a7ecd","entire":{"status":"ended","checkpoints":6,"only":true}},{"harness":"codex","id":"b7460ac0","entire":{"status":"ended","checkpoints":1,"only":true}},{"harness":"cursor","id":"c0ffee00","entire":{"status":"idle","checkpoints":2,"only":true}}]'
	assert_only_read_only_calls
}

@test "sessions without Entire lists what the transcripts say" {
	FAKE_ENTIRE=disabled run --separate-stderr jouet sessions --all --since 2020-01-01
	assert_success
	assert_output 'no live agent sessions on this machine'
}

@test "read --why says which session and checkpoint wrote the lines" {
	run --separate-stderr jouet read wait_port.sh --range 2:5 --why
	assert_success
	assert_line --partial '4	'
	assert_line 'origin (entire why): 3 lines (3-5) by Claude Code session 8d2ac41d, checkpoint 01M66H17P11G (commit 12fa1abf): "Yes, fix both flaky tests in a separate PR off main. GH_TOKEN=[redacted]"; 1 line by people (1 commit)'
	refute_output --partial ghp_

	run --separate-stderr jouet read wait_port.sh --range 3:4 --why --again --json
	run jq -c '.sections[0].origin.agent[0] | {checkpoint_id, session_id, lines}' <<<"$output"
	assert_output '{"checkpoint_id":"01M66H17P11G5CYSP65VHNQVMB","session_id":"8d2ac41d-2f33-454f-8d12-c98a0770a25b","lines":[3,4]}'
	assert_only_read_only_calls
}

@test "read --why needs a range or symbol, and says when Entire is not there" {
	run --separate-stderr jouet read wait_port.sh --why
	assert_failure 2
	assert_equal "$stderr" 'jouet: --why goes with --symbol or --range'

	FAKE_ENTIRE=disabled run --separate-stderr jouet read wait_port.sh --range 1:2 --why
	assert_success
	assert_line 'note: --why: Entire is not set up here; it needs `entire` on PATH, the repository enabled (`entire status`) and a working login'

	# Without --why, entire is not asked.
	: >"$FAKE_ENTIRE_LOG"
	run --separate-stderr jouet read wait_port.sh --range 1:2 --again
	assert_success
	run cat "$FAKE_ENTIRE_LOG"
	assert_output ''
}

@test "edit --why warns about lines an earlier session wrote, and not this session's own" {
	run --separate-stderr jouet edit replace wait_port.sh --old '|| true)' --new '|| :)' --dry-run --why
	assert_success
	assert_line 'warning: this edit changes a line of wait_port.sh that an earlier session wrote: 1 line (4) by Claude Code session 8d2ac41d, checkpoint 01M66H17P11G (commit 12fa1abf): "Yes, fix both flaky tests in a separate PR off main. GH_TOKEN=[redacted]". Check that the change is intended (`entire why wait_port.sh:4`)'

	run --separate-stderr jouet edit replace wait_port.sh --old '|| true)' --new '|| :)' --dry-run --why --json
	run jq -c '.earlier_sessions[] | {path, lines, checkpoint_id}' <<<"$output"
	assert_output '{"path":"wait_port.sh","lines":[4],"checkpoint_id":"01M66H17P11G5CYSP65VHNQVMB"}'

	CLAUDE_CODE_SESSION_ID=8d2ac41d-2f33-454f-8d12-c98a0770a25b run --separate-stderr jouet edit replace wait_port.sh --old '|| true)' --new '|| :)' --why
	assert_success
	refute_output --partial warning
	assert_content wait_port.sh $'#!/bin/sh\nport=""\nfor _ in $(seq 1 100); do\n\tport=$(cat port 2>/dev/null || :)\ndone\necho "$port"\n'
	assert_only_read_only_calls
}

@test "edit --why writes the edit when entire why is too slow, and says so" {
	FAKE_ENTIRE_SLOW=1 JOUET_ENTIRE_WHY_TIMEOUT_MS=300 run --separate-stderr jouet edit replace wait_port.sh --old 'echo' --new 'printf "%s\n"' --why
	assert_success
	assert_line 'note: --why: `entire why wait_port.sh --json` took longer than 300ms, so jouet stopped waiting'
	assert_line --partial '1 of 1 file written'
}

@test "edit without --why never runs entire" {
	run --separate-stderr jouet edit replace wait_port.sh --old 'echo' --new 'printf "%s\n"'
	assert_success
	run cat "$FAKE_ENTIRE_LOG"
	assert_output ''
}

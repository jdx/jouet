#!/usr/bin/env bats

# `jouet pr threads` changing Entire findings through a fake `entire` that
# replays `--json` output recorded from jdx/mise#13958 (trail #424: two open
# findings, resolved and dismissed stale ones) and logs every call. A
# branch's findings need nothing from GitHub, which is an address nothing
# listens on, so no test here touches the network. tests/entire.rs covers
# the commands that read GitHub too, and test/entire.bats Entire's sessions.

BRANCH=claude/project-thread-htk77e-5-experimental
OPEN=01M4C997EAAK13R52CVH7GYY7M
RESOLVED=01M4C58QTFMH4RQH9V760F97GZ
DISMISSED=01M45K987D892K9QM9CMYBSTC4

setup() {
	load "test_helper/common_setup"
	_common_setup
	export JOUET_GITHUB_API_URL="http://127.0.0.1:9"
	export JOUET_GITHUB_TOKEN="test-token"
	unset JOUET_REPLY_FOOTER JOUET_ENTIRE JOUET_ENTIRE_TIMEOUT_MS
	export FAKE_ENTIRE_FIXTURES="$PROJECT_ROOT/test/fixtures/entire"
	fake_entire
}

# A fake `entire` on PATH. FAKE_ENTIRE_AUTH=out makes it logged out,
# FAKE_ENTIRE_SLEEP makes every call that slow, and FAKE_ENTIRE_FAIL makes
# writes fail with that message, as the real one fails: a line on stderr,
# exit 1.
fake_entire() {
	cat >"$BATS_TEST_TMPDIR/bin/entire" <<'SH'
#!/bin/sh
printf '%s\n' "$*" >>"$BATS_TEST_TMPDIR/entire.log"
[ -n "${FAKE_ENTIRE_SLEEP:-}" ] && sleep "$FAKE_ENTIRE_SLEEP"
fx="$FAKE_ENTIRE_FIXTURES"
case "$*" in
"auth status --json")
	if [ "${FAKE_ENTIRE_AUTH:-}" = out ]; then
		cat "$fx/auth-logged-out.json"
	else
		cat "$fx/auth-ok.json"
	fi
	;;
"trail show --branch claude/project-thread-htk77e-5-experimental --json --repo gh/jdx/mise")
	cat "$fx/show.json"
	;;
"trail show --branch "*" --repo gh/jdx/elsewhere")
	echo "jdx/elsewhere is not onboarded to Entire, or is not visible to your login. If it should be onboarded, run 'entire repo mirror add'." >&2
	exit 1
	;;
"trail show --branch "*)
	echo "no trail found for current branch \"$4\"" >&2
	echo "hint: run 'entire trail create' or 'entire trail list --status any'" >&2
	exit 1
	;;
"trail list --status any --json -n 100 --repo gh/jdx/mise")
	echo '[]'
	;;
"trail finding --trail 424 --json -n 100 --repo gh/jdx/mise --status any --freshness any --include-dismissed")
	cat "$fx/finding-any.json"
	;;
"trail finding resolve 424 "* | "trail finding dismiss 424 "* | "trail finding reopen 424 "*)
	if [ -n "${FAKE_ENTIRE_FAIL:-}" ]; then
		echo "Error: $FAKE_ENTIRE_FAIL" >&2
		exit 1
	fi
	echo "Finding $5 updated"
	;;
*)
	echo "fake entire: unexpected call: $*" >&2
	exit 64
	;;
esac
SH
	chmod +x "$BATS_TEST_TMPDIR/bin/entire"
}

# The calls that changed a finding, one per line.
writes() {
	grep -E '^trail finding (resolve|dismiss|reopen) ' "$BATS_TEST_TMPDIR/entire.log" || true
}

@test "resolving a finding records the reply as the reason, and leaves GitHub alone" {
	run --separate-stderr jouet pr threads resolve entire:01M4C997EA "$BRANCH" --repo jdx/mise --reply "Fixed: uninstall tiny first"

	assert_success
	assert_line "[entire:01M4C997EA] resolved on Entire trail #424"
	assert_line "Entire findings are separate from GitHub threads: no thread was changed by these."
	assert_equal "$(writes)" "trail finding resolve 424 $OPEN -m Fixed: uninstall tiny first --repo gh/jdx/mise"
}

@test "dismissing needs a reason, and is refused before entire is asked" {
	run --separate-stderr jouet pr threads resolve entire:01M4C997EA "$BRANCH" --repo jdx/mise --dismiss --json

	assert_failure 2
	assert_equal "$(jq -r '.error.code' <<<"$output")" "usage"
	assert_equal "$(jq -r '.error.message' <<<"$output")" "dismissing entire:01M4C997EA needs a reason; nothing was changed"
	[ ! -e "$BATS_TEST_TMPDIR/entire.log" ]
}

@test "a reply with --dismiss dismisses, the reply being the reason" {
	run --separate-stderr jouet pr threads reply "entire:$OPEN" "$BRANCH" --repo jdx/mise --dismiss --body "Not reachable: tiny is never installed here" --json

	assert_success
	assert_equal "$(jq -c '.entire' <<<"$output")" "{\"trail\":424,\"results\":[{\"id\":\"entire:$OPEN\",\"finding\":\"$OPEN\",\"dismissed\":true}]}"
	assert_equal "$(jq -r '.pr // "none"' <<<"$output")" "none"
	assert_equal "$(writes)" "trail finding dismiss 424 $OPEN -m Not reachable: tiny is never installed here --repo gh/jdx/mise"
}

@test "a finding takes no reply of its own" {
	run --separate-stderr jouet pr threads reply entire:01M4C997EA "$BRANCH" --repo jdx/mise --body "Looking"

	assert_failure 2
	assert_equal "$stderr" "jouet: entire:01M4C997EA is an Entire finding, which takes no reply of its own; nothing was changed
hint: add --resolve or --dismiss: the reply becomes the reason recorded on Entire"
}

@test "resolved and dismissed findings are left as they are, and unresolve reopens" {
	run --separate-stderr jouet pr threads resolve "entire:${RESOLVED:0:10}" "entire:${DISMISSED:0:10}" "$BRANCH" --repo jdx/mise

	assert_success
	assert_line "[entire:${RESOLVED:0:10}] already resolved on Entire trail #424"
	assert_line "[entire:${DISMISSED:0:10}] already dismissed on Entire trail #424"
	assert_equal "$(writes)" ""

	run --separate-stderr jouet pr threads unresolve "entire:${RESOLVED:0:10}" "$BRANCH" --repo jdx/mise

	assert_success
	assert_line "[entire:${RESOLVED:0:10}] reopened on Entire trail #424"
	assert_equal "$(writes)" "trail finding reopen 424 $RESOLVED --repo gh/jdx/mise"
}

@test "a dry run prints the entire command and changes nothing" {
	run --separate-stderr jouet pr threads resolve entire:01M4C997EA "$BRANCH" --repo jdx/mise --reply "It's fixed" --dry-run

	assert_success
	assert_line "[entire:01M4C997EA] would resolve on Entire trail #424"
	assert_line "  | entire trail finding resolve 424 $OPEN -m 'It'\\''s fixed' --repo gh/jdx/mise"
	assert_equal "$(writes)" ""
}

@test "apply takes findings by handle" {
	run --separate-stderr jouet pr threads apply - "$BRANCH" --repo jdx/mise <<JSON
[{"id": "entire:01M4C997EA", "reply": "Fixed", "resolve": true},
 {"id": "entire:01M4C997DZ", "reply": "Duplicate of the first", "dismiss": true}]
JSON

	assert_success
	assert_equal "$(writes)" "trail finding resolve 424 $OPEN -m Fixed --repo gh/jdx/mise
trail finding dismiss 424 01M4C997DZH5KQRP31NW93NQHR -m Duplicate of the first --repo gh/jdx/mise"
}

@test "an unknown handle is not found, and nothing changes" {
	run --separate-stderr jouet pr threads resolve entire:01M4C997EA entire:01ZZZZZZZZ "$BRANCH" --repo jdx/mise

	assert_failure 3
	assert_equal "$stderr" "jouet: no finding entire:01ZZZZZZZZ on Entire trail #424; nothing was changed"
	assert_equal "$(writes)" ""
}

@test "a branch no trail tracks is not found" {
	run --separate-stderr jouet pr threads resolve entire:01M4C997EA feat/elsewhere --repo jdx/mise

	assert_failure 3
	assert_equal "$stderr" "jouet: no Entire trail tracks jdx/mise:feat/elsewhere; nothing was changed"
}

@test "a repository Entire does not know has no trail, quietly" {
	run --separate-stderr jouet pr threads resolve entire:01M4C997EA main --repo jdx/elsewhere

	assert_failure 3
	assert_equal "$stderr" "jouet: no Entire trail tracks jdx/elsewhere:main; nothing was changed"
}

@test "a write entire refuses stops there and exits 5" {
	FAKE_ENTIRE_FAIL="finding is locked" run --separate-stderr jouet pr threads resolve entire:01M4C997EA entire:01M4C997DZ "$BRANCH" --repo jdx/mise

	assert_failure 5
	assert_line "[entire:01M4C997EA] resolve failed: finding is locked"
	assert_line "[entire:01M4C997DZ] resolve failed: not tried after an earlier failure"
	assert_line "stopped at a failure; re-running skips what was already posted and resolved"
}

@test "without entire on PATH, changing a finding is refused as a precondition" {
	rm "$BATS_TEST_TMPDIR/bin/entire"
	PATH="$BATS_TEST_TMPDIR/bin:/usr/bin:/bin" run --separate-stderr jouet pr threads resolve entire:01M4C997EA "$BRANCH" --repo jdx/mise --reply Fixed --json

	assert_failure 4
	assert_equal "$(jq -r '.error.code' <<<"$output")" "precondition"
	assert_equal "$(jq -r '.error.message' <<<"$output")" "entire is not on PATH, so Entire findings cannot be changed; nothing was changed"
}

@test "logged out of entire, changing a finding is refused as a precondition" {
	FAKE_ENTIRE_AUTH=out run --separate-stderr jouet pr threads resolve entire:01M4C997EA "$BRANCH" --repo jdx/mise --reply Fixed

	assert_failure 4
	assert_equal "$stderr" "jouet: entire is not logged in (see \`entire auth status\`), so Entire findings cannot be changed; nothing was changed
hint: findings are changed through the entire CLI: install it and run \`entire login\`"
	assert_equal "$(writes)" ""
}

@test "an entire that hangs is cut off at JOUET_ENTIRE_TIMEOUT_MS" {
	FAKE_ENTIRE_SLEEP=5 JOUET_ENTIRE_TIMEOUT_MS=1000 run --separate-stderr jouet pr threads resolve entire:01M4C997EA "$BRANCH" --repo jdx/mise --reply Fixed

	assert_failure 4
	assert_equal "$stderr" "jouet: entire auth status: entire took longer than 1s, so Entire findings cannot be changed; nothing was changed
hint: findings are changed through the entire CLI: install it and run \`entire login\`"
}

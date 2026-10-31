#!/usr/bin/env bats
# Messages name commands in backquotes, which the tests match literally.
# shellcheck disable=SC2016

# A repository Entire hosts itself, as an agent meets it in a checkout whose
# remote is entire://: the trail is the PR, its discussions are the threads,
# and its check runs and gates are the checks. A fake `entire` on PATH
# replays `--json` output recorded from et/jdx/jouet-entire-forge-test
# (crates/jouet-github/src/entire_forge/fixtures) and logs every call, so nothing here talks to
# Entire. FAKE_TRAIL=failing serves the trail with failing gates and checks.

TESTS=01M69HAFTFES527BAJQWFDZ5YA
NAMING=01M69GVB1SNG4YRX6R3J5ZHSDP
REPO=et/jdx/jouet-entire-forge-test

setup() {
	load "test_helper/common_setup"
	_common_setup
	export JOUET_GITHUB_API_URL="http://127.0.0.1:9"
	unset JOUET_ENTIRE JOUET_ENTIRE_TIMEOUT_MS JOUET_REPLY_FOOTER
	export FAKE_ENTIRE_FIXTURES="$PROJECT_ROOT/crates/jouet-github/src/entire_forge/fixtures"
	export FAKE_ENTIRE_AUTH="$PROJECT_ROOT/test/fixtures/entire/auth-ok.json"
	fake_entire
	git init -q -b feat/subtract .
	git remote add origin "entire://aws-us-east-2.entire.io/$REPO"
}

fake_entire() {
	cat >"$BATS_TEST_TMPDIR/bin/entire" <<'SH'
#!/bin/sh
printf '%s\n' "$*" >>"$BATS_TEST_TMPDIR/entire.log"
fx="$FAKE_ENTIRE_FIXTURES"
repo="--repo et/jdx/jouet-entire-forge-test"
case "$*" in
"auth status --json") cat "$FAKE_ENTIRE_AUTH" ;;
"trail show --branch feat/subtract --json $repo" | "trail show 1 --json $repo")
	if [ "${FAKE_TRAIL:-}" = failing ]; then cat "$fx/show-failing.json"; else cat "$fx/show.json"; fi
	;;
"trail show 2 --json $repo") cat "$fx/show-2.json" ;;
"trail show 9 --json $repo")
	echo "no trail \"9\" found in et/jdx/jouet-entire-forge-test (run 'entire trail list --status any')" >&2
	exit 1
	;;
"trail comment list --trail 1 --json $repo") cat "$fx/comment-list.json" ;;
"trail comment list --trail 2 --json $repo") cat "$fx/comment-list-2.json" ;;
"trail comment show "*" --trail 1 --json $repo") cat "$fx/comment-$4.json" ;;
"trail approvals "[12]" --json $repo") cat "$fx/approvals.json" ;;
"trail list --author jdx --status open,draft --json -n 100 $repo") cat "$fx/list-mine.json" ;;
"trail finding --trail 1 --json -n 100 $repo") cat "$fx/finding-open.json" ;;
"trail finding --trail 1 --json -n 100 $repo --status any --freshness any --include-dismissed") cat "$fx/finding-any.json" ;;
"trail comment reply "* | "trail comment resolve "* | "trail comment unresolve "*)
	if [ -n "${FAKE_ENTIRE_FAIL:-}" ]; then
		echo "Error: $FAKE_ENTIRE_FAIL" >&2
		exit 1
	fi
	;;
*)
	echo "fake entire: unexpected call: $*" >&2
	exit 64
	;;
esac
SH
	chmod +x "$BATS_TEST_TMPDIR/bin/entire"
}

# The calls that changed something, one per line.
writes() {
	grep -E '^trail (comment (reply|resolve|unresolve)|finding (resolve|dismiss|reopen)) ' "$BATS_TEST_TMPDIR/entire.log" || true
}

@test "an entire:// remote is the repository, and its branch's trail is the PR" {
	run --separate-stderr jouet pr status --json

	assert_success
	local pr
	pr=$(jq -c '.prs[0]' <<<"$output")
	assert_equal "$(jq -r '"\(.forge) \(.host) \(.repo)#\(.number) \(.state)"' <<<"$pr")" \
		"entire entire.com jdx/jouet-entire-forge-test#1 open"
	assert_equal "$(jq -r '.url' <<<"$pr")" "https://entire.com/et/jdx/jouet-entire-forge-test/trails/1"
	assert_equal "$(jq -r '.head_sha' <<<"$pr")" "e971859eed"
	assert_equal "$(jq -r '.unresolved_threads' <<<"$pr")" "1"
	assert_equal "$(jq -c '.checks' <<<"$pr")" '{"pass":1}'
	# No other repository was asked about, and nothing was written.
	# -E: macOS grep has no \| in a basic regex.
	run grep -c -v -E -e "--repo $REPO\$|^auth status" "$BATS_TEST_TMPDIR/entire.log"
	assert_output 0
	assert_equal "$(writes)" ""
}

@test "a trail URL and entire.com/PROJECT/NAME#N name the same trail" {
	cd "$BATS_TEST_TMPDIR" || return 1
	run --separate-stderr jouet pr status https://entire.com/et/jdx/jouet-entire-forge-test/trails/1 --json
	assert_success
	assert_equal "$(jq -r '.prs[0].number' <<<"$output")" "1"

	run --separate-stderr jouet pr status entire.com/jdx/jouet-entire-forge-test#1
	assert_success
	assert_line --index 0 --regexp '^#1 open feat: add subtract +ok +ci 1/1 +threads 1 +review -$'
}

@test "failing gates read as an Entire Gates check, and approvals as the review" {
	FAKE_TRAIL=failing run --separate-stderr jouet pr status --json

	assert_success
	local pr
	pr=$(jq -c '.prs[0]' <<<"$output")
	assert_equal "$(jq -c '.checks' <<<"$pr")" '{"fail":2,"pending":1,"failing":["buildkite/test","Entire Gates"]}'
	assert_equal "$(jq -r '.review' <<<"$pr")" "review_required"
	assert_equal "$(jq -r '.merge_state' <<<"$pr")" "blocked"
	assert_equal "$(jq -r '[.attention[].code] | join(",")' <<<"$pr")" \
		"checks_failing,unresolved_threads,entire_findings,behind_base"
	assert_equal "$(jq -r '.entire.trail' <<<"$pr")" "1"
	# The approvals gate is the review, not one of the gates.
	assert_equal "$(jq -r '[.entire.gates[].gate] | join(",")' <<<"$pr")" "findings,up_to_date"
	assert_equal "$(jq -r '.entire.findings[0].handle' <<<"$pr")" "entire:01M69HBX3M"

	FAKE_TRAIL=failing run --separate-stderr jouet pr status
	assert_line --index 1 "  entire: findings: Unresolved findings: 1 medium severity; up to date: 1 commit behind (trail #1)"
	assert_line --index 2 --partial "[entire:01M69HBX3M] main.rs:10  low  main prints both results"
}

@test "pr threads lists the discussions as threads, then the open findings" {
	run --separate-stderr jouet pr threads --all

	assert_success
	assert_line --index 0 "jdx/jouet-entire-forge-test#1  1 unresolved of 2 threads  showing 2"
	assert_output --partial "discussion  jdx"
	assert_output --partial "  Tests"
	assert_output --partial "Could subtract get a test"
	assert_output --partial "Keeping subtract: the other helpers are spelled out too."
	assert_output --partial "These are Entire's, not the trail's discussions: resolving one leaves the other as it was."
	assert_output --partial "[entire:01M69HBX3M] main.rs:10  low"
}

@test "a reply goes into the discussion and resolves it, through the entire CLI" {
	run --separate-stderr jouet pr threads --json
	local id
	id=$(jq -r '.threads[0].id' <<<"$output")

	run --separate-stderr jouet pr threads reply "$id" --body "Added a test." --resolve

	assert_success
	assert_output "[$id] replied, resolved  https://entire.com/et/jdx/jouet-entire-forge-test/trails/1"
	assert_equal "$(writes)" "trail comment reply $TESTS -m Added a test. --trail 1 --repo $REPO
trail comment resolve $TESTS --trail 1 --repo $REPO"
}

@test "unresolving a discussion is a write of its own" {
	run --separate-stderr jouet pr threads --resolved --json
	local id
	id=$(jq -r '.threads[0].id' <<<"$output")

	run --separate-stderr jouet pr threads unresolve "$id"

	assert_success
	assert_equal "$(writes)" "trail comment unresolve $NAMING --trail 1 --repo $REPO"
}

@test "a reply Entire refuses exits 5 with its message" {
	run --separate-stderr jouet pr threads --json
	local id
	id=$(jq -r '.threads[0].id' <<<"$output")
	export FAKE_ENTIRE_FAIL="trail API: API error: forbidden (status 403)"

	run --separate-stderr jouet pr threads reply "$id" --body "x"

	assert_failure 5
	assert_output --partial "trail API: API error: forbidden (status 403)"
}

@test "--mine lists the trails entire's login opened" {
	run --separate-stderr jouet pr status --mine --json

	assert_success
	assert_equal "$(jq -r '[.prs[].number] | join(",")' <<<"$output")" "1,2"
}

@test "a trail that does not exist is not found" {
	run --separate-stderr jouet pr status 9 --json

	assert_failure 3
	assert_equal "$(jq -r '.not_found[0]' <<<"$output")" "jdx/jouet-entire-forge-test#9"
}

@test "wait ci follows the trail's check runs and gates" {
	run --separate-stderr jouet wait ci --timeout 10s

	assert_success
	assert_output --regexp '^PASS jdx/jouet-entire-forge-test#1 @ e971859  1 pass, 0 fail, 0 skipped, 0 pending'
}

@test "wait ci fails on a failed check run, and on findings, but not on the branch being behind" {
	FAKE_TRAIL=failing run --separate-stderr jouet wait ci --timeout 10s --json

	assert_failure 1
	assert_equal "$(jq -r '[.checks[] | select(.state == "fail") | .name] | sort | join(",")' <<<"$output")" \
		"Entire Gates,buildkite/test"
	assert_equal "$(jq -r '.checks[] | select(.name == "buildkite/test") | .url' <<<"$output")" \
		"https://buildkite.com/jdx/jouet-entire-forge-test/builds/7"
}

@test "pr next names no merge command, since the entire CLI has none" {
	run --separate-stderr jouet pr next --json

	assert_failure 10
	assert_equal "$(jq -r '.prs[0].action' <<<"$output")" "unresolved_threads"

	cd "$BATS_TEST_TMPDIR" || return 1
	run --separate-stderr jouet pr next entire.com/jdx/jouet-entire-forge-test#2
	assert_success
	assert_line --index 0 "#2 done ready_to_merge: Checks pass (1/1), no review required, no unresolved threads; merge it on entire.com: https://entire.com/et/jdx/jouet-entire-forge-test/trails/2"
}

@test "what Entire's CLI cannot do is refused with exit 4 before anything runs" {
	run --separate-stderr jouet pr land --yes
	assert_failure 4
	[[ $stderr == *"the entire CLI has no merge command"* ]]

	run --separate-stderr jouet wait run 7
	assert_failure 4
	[[ $stderr == *"Entire gives a trail's checks but no runs or jobs of its own"* ]]

	run --separate-stderr jouet ci explain
	assert_failure 4

	run --separate-stderr jouet wait ci --required --timeout 5s
	assert_failure 4
	[[ $stderr == *"--required would select none; name them with --check"* ]]

	run --separate-stderr jouet pr status --search "is:open"
	assert_failure 4
	[[ $stderr == *"is on Entire, which has no equivalent"* ]]
	assert_equal "$(writes)" ""
}

@test "without entire, an Entire-hosted repository says why it cannot be read" {
	rm "$BATS_TEST_TMPDIR/bin/entire"
	export PATH="$BATS_TEST_TMPDIR/bin:/usr/bin:/bin"

	run --separate-stderr jouet pr status --json

	assert_failure
	assert_output --partial "Entire-hosted repositories are read through \`entire\`, and entire is not on PATH"
}

@test "a GitHub repository served through Entire is still GitHub's" {
	git remote set-url origin entire://aws-us-east-2.entire.io/gh/jdx/mise

	run --separate-stderr jouet pr status 1 --settle 0 --json

	assert_failure 5
	# GitHub was asked (and nothing listens there); Entire was not.
	assert_output --partial "127.0.0.1:9"
	[[ ! -s "$BATS_TEST_TMPDIR/entire.log" ]]
}

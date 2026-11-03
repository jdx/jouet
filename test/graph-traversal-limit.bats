#!/usr/bin/env bats

setup() {
	load "test_helper/common_setup"
	_common_setup
	unset CODEX_THREAD_ID JOUET_SESSION_ID
	export XDG_CACHE_HOME="$BATS_TEST_TMPDIR/cache"
	git init -q .
	printf 'def target():\n    return 1\n' >calls.py
	for i in $(seq 1 2001); do
		printf 'def caller_%s():\n    return target()\n' "$i" >>calls.py
	done
	jouet graph index >/dev/null
}

@test "read and edit report unavailable callers when graph traversal exceeds the limit" {
	run --separate-stderr jouet read calls.py --symbol target --again --json
	assert_success
	assert_equal "$(jq -r '.notes[] | select(contains("callers of target unavailable"))' <<<"$output")" \
		'callers of target unavailable: graph traversal exceeds 2000 symbols; narrow the query or depth'
	run --separate-stderr jouet read calls.py --symbol target --again
	assert_success
	assert_line --partial 'note: callers of target unavailable: graph traversal exceeds 2000 symbols'

	run --separate-stderr jouet edit --dry-run <<'JOUET_EDIT'
=== calls.py#target
def target(value):
    return value
JOUET_EDIT
	assert_success
	assert_line --partial 'warning: the signature of target changed; callers unavailable: graph traversal exceeds 2000 symbols'

	run --separate-stderr jouet edit --json <<'JOUET_EDIT'
=== calls.py#target
def target(value):
    return value
JOUET_EDIT
	assert_success
	assert_equal "$(jq -r '.callers[0].error' <<<"$output")" \
		'graph traversal exceeds 2000 symbols; narrow the query or depth'
}

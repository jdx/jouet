#!/usr/bin/env bats

setup() {
	load "test_helper/common_setup"
	_common_setup
	unset JOUET_WIND JOUET_SESSION_ID CODEX_THREAD_ID
	export CODEX_HOME="$HOME/.codex"
	git init -q .
	mkdir -p "$CODEX_HOME/sessions/2026/01/01"
	TRANSCRIPT="$CODEX_HOME/sessions/2026/01/01/rollout-2026-01-01T00-00-00-01a00000-0000-7000-8000-000000000001.jsonl"
	jq -nc --arg cwd "$PWD" '{type:"session_meta",payload:{id:"01a00000-0000-7000-8000-000000000001",cwd:$cwd,originator:"codex_cli_rs"}}' >"$TRANSCRIPT"
	printf '%s\n' '{"timestamp":"2026-01-01T00:00:01Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"run the tests"}]}}' >>"$TRANSCRIPT"
}

@test "scheduled review claims and records only an unchanged snapshot" {
	run --separate-stderr jouet wind plan --root "$PWD" --idle 0s --json
	assert_success
	snapshot=$(jq -r '.candidates[0].snapshot' <<<"$output")
	assert_equal "$(jq '.eligible' <<<"$output")" 1

	run --separate-stderr jouet wind claim "$snapshot" --root "$PWD" --idle 0s --json
	assert_success
	lease=$(jq -r '.lease' <<<"$output")
	project=$(jq -r '.project' <<<"$output")
	transcript=$(jq -r '.transcript' <<<"$output")
	run --separate-stderr jouet wind claim "$snapshot" --root "$PWD" --idle 0s
	assert_failure 4
	run --separate-stderr jouet wind review "$transcript" --project "$project" --lease "$lease"
	assert_success
	assert_output --partial "jouet wind propose --lease $lease"
	run --separate-stderr jouet wind review "$transcript" --project / --lease "$lease"
	assert_failure 2

	run --separate-stderr jouet wind propose --lease "$lease" --json <<'JSON'
{"session":"wrong","proposals":[{"kind":"nothing"}]}
JSON
	assert_failure 4
	run --separate-stderr jouet wind propose --lease "$lease" --json <<'JSON'
{"session":"01a00000-0000-7000-8000-000000000001","proposals":[{"kind":"nothing"}]}
JSON
	assert_success
	assert_equal "$(jq -r '.scheduled_reviewed' <<<"$output")" true
	run --separate-stderr jouet wind plan --root "$PWD" --idle 0s --json
	assert_equal "$(jq '.reviewed' <<<"$output")" 1
	assert_equal "$(jq '.eligible' <<<"$output")" 0

	jq -nc --arg cwd "$PWD" '{type:"turn_context",payload:{cwd:$cwd}}' >>"$TRANSCRIPT"
	run --separate-stderr jouet wind plan --root "$PWD" --idle 0s --json
	assert_equal "$(jq '.eligible' <<<"$output")" 1
}

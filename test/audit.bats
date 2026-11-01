#!/usr/bin/env bats

# `jouet audit` over the fixture transcripts in crates/jouet-audit/tests/fixtures:
# a Claude Code session with a subagent, a Codex session whose exec cells use
# bare JavaScript keys, a zstd-compressed archived rollout, and one cut off
# partway through its second zstd frame.

setup() {
	load "test_helper/common_setup"
	_common_setup
	cp -R "$PROJECT_ROOT/crates/jouet-audit/tests/fixtures/home/." "$HOME/"
	mv "$HOME/claude" "$HOME/.claude"
	mv "$HOME/codex" "$HOME/.codex"
	unset CLAUDE_CONFIG_DIR CODEX_HOME
}

@test "the report ranks categories and names what would cut them" {
	run --separate-stderr jouet audit --display plain

	assert_success
	assert_equal "$stderr" ""
	assert_line --index 0 --regexp '^jouet audit: 4 sessions \(read 5 files, [0-9]+ KB, in [0-9.]+s\)$'
	assert_line 'Claude Code: 1 session (2 transcripts with subagents), 8 requests, 10 tool calls (1 failed)'
	assert_line --regexp '^  CI polling +24\.3% +41k +2 +2 +34s +41 B$'
	assert_line --partial '- CI polling (24.3%): jouet wait ci'
	assert_line '    estimate: 21k tokens (12.4%), counting requests that polled CI right after a request that did; a wait would have made each run of polls one call'
	assert_line --regexp '^  process polling +51\.2% '
	assert_output --partial 'linked checks in'
	refute_output --partial '- process polling ('
	assert_line --partial '- stale-token prefix: 1 call in 1 session, 12k tokens'
	assert_line --partial "1 file could not be read in full:"
	assert_line --partial "000000000003.jsonl.zst: incomplete frame (counted up to the error)"
}

@test "only shapes are shown unless examples are asked for, and those are redacted" {
	run --separate-stderr jouet audit --display plain
	assert_success
	assert_output --partial 'GH_TOKEN= gh pr view <n> --repo <path> --json <v>'
	refute_output --partial 'acme/app'
	refute_output --partial 'FAKE'

	run --separate-stderr jouet audit --display plain --examples
	assert_success
	assert_output --partial 'e.g. gh pr checks 12 --repo acme/app'
	assert_output --partial "Authorization: Bearer <redacted>"
	assert_output --partial '<heredoc: 3 lines>'
	refute_output --partial 'FAKE'
	refute_output --partial 'write_text'
}

@test "--json prints one minified document with empty fields left out" {
	run --separate-stderr jouet audit --json --display plain

	assert_success
	[[ "$(jq -r '.harnesses | map(.harness) | join(",")' <<<"$output")" == "claude,codex" ]]
	[[ "$(jq '.scan.files' <<<"$output")" == 5 ]]
	[[ "$(jq -r '.harnesses[1].categories[0].category' <<<"$output")" == process_polling ]]
	[[ "$(jq '.harnesses[1].process_waits.linked_checks > 0' <<<"$output")" == true ]]
	[[ "$(jq '[.harnesses[1].suggestions[]? | select(.category == "process_polling")] | length' <<<"$output")" == 0 ]]
	[[ "$(jq '.harnesses[0].totals.input_tokens' <<<"$output")" == 169000 ]]
	[[ "$(jq '[.harnesses[0].categories[] | select(.category == "reply") | has("calls")] | .[0]' <<<"$output")" == false ]]
	[[ "$(jq -r '.scan.unreadable[0].partial' <<<"$output")" == true ]]
	[[ "$output" != *$'\n'* ]]
}

@test "--harness, --project and --top narrow the report, and say what they leave out" {
	run --separate-stderr jouet audit --display plain --harness codex --project /work/app/../other/.
	assert_success
	assert_line --index 0 --partial 'jouet audit: 2 sessions in /work/other'
	refute_output --partial 'Claude Code'

	run --separate-stderr jouet audit --display plain --harness claude --top 2
	assert_success
	assert_line '  … 5 more categories (--top 0 lists all)'
}

@test "--since keeps requests made after it" {
	run --separate-stderr jouet audit --json --display plain --harness claude --since 2026-10-30T10:01:42Z
	assert_success
	[[ "$(jq '.harnesses[0].totals.requests' <<<"$output")" == 3 ]]

	run --separate-stderr jouet audit --display plain --since 2099-01-01
	assert_failure 3
	assert_equal "$stderr" $'jouet: no transcripts matched under '"$HOME"$'/.claude/projects and '"$HOME"$'/.codex\nhint: Claude Code\'s directory is $CLAUDE_CONFIG_DIR or ~/.claude, Codex\'s is $CODEX_HOME or ~/.codex'
}

@test "usage errors exit 2, and no transcripts at all exit 3" {
	run --separate-stderr jouet audit --harness cursor
	assert_failure 2
	assert_equal "$stderr" "jouet: --harness must be claude, codex or all, not \`cursor\`"

	run --separate-stderr jouet audit --since yesterday --json
	assert_failure 2
	[[ "$(jq -r '.error.code' <<<"$output")" == usage ]]

	run --separate-stderr jouet audit --since 99999999999999d
	assert_failure 2
	assert_equal "$stderr" "jouet: --since: too far back"

	rm -rf "$HOME/.claude" "$HOME/.codex"
	run --separate-stderr jouet audit --json
	assert_failure 3
	[[ "$(jq -r '.error.code' <<<"$output")" == not_found ]]
}

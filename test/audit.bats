#!/usr/bin/env bats
# shellcheck disable=SC2016 # backticks in expected output are Markdown

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
	assert_output --partial 'recorded shell-call time'
	assert_line --regexp '^  CI polling +24\.3% +41k +2 +2 +34s +41 B$'
	assert_line --partial '- CI polling (24.3%): jouet wait ci'
	refute_output --partial 'counting requests that polled CI right after a request that did'
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
	[[ "$(jq -c '[.harnesses[0].suggestions[] | select(.category == "ci_polling") | has("saving")]' <<<"$output")" == '[false]' ]]
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

@test "the fixed context costs each definition per request and names what removes an unused one" {
	run --separate-stderr jouet audit --display plain --harness claude
	assert_success
	assert_line --partial '  fixed context: tool, skill and MCP definitions sent with every request'
	assert_line '  137 per request on average, over 7 requests in 1 transcript: 961 tokens, 0.6% of input'
	assert_line --regexp '^  Bash +tool +27 +1 +186 +7$'
	assert_line '  - Workflow (tool, 37 per request, 259 tokens): permissions.deny: ["Workflow"]'
	assert_line '  - Artifact (tool, 14 per request, 96 tokens): enableArtifact: false'
	assert_line '  - cloudflare (plugin, 15 per request, 105 tokens): enabledPlugins: {"cloudflare@<marketplace>": false}'
	assert_line '  - computer-use (MCP server, 10 per request, 72 tokens): disconnect the MCP server'
	refute_line --partial '  - Bash ('

	run --separate-stderr jouet audit --json --display plain --harness claude
	assert_success
	[[ "$(jq -c '.harnesses[0].fixed_context | [.transcripts, .requests, .periods[0].start]' <<<"$output")" == '[1,7,"2026-10-26"]' ]]
	[[ "$(jq -r '.harnesses[0].fixed_context.items[] | select(.name == "Read") | "\(.calls) \(.remove // "kept")"' <<<"$output")" == "1 kept" ]]
}

@test "build/test/lint names jouet test" {
	run --separate-stderr jouet audit --display plain --harness claude
	assert_success
	assert_line --partial '- build/test/lint (13.0%): jouet test — `--cmd` keeps a run'"'"'s whole log out of context'
	refute_output --partial 'no command for this'
}

@test "--until keeps requests made before it" {
	run --separate-stderr jouet audit --json --display plain --harness claude --until 2026-10-30T10:01:00Z
	assert_success
	[[ "$(jq '.harnesses[0].totals.requests' <<<"$output")" == 3 ]]
	[[ "$(jq -r '.until' <<<"$output")" == 2026-10-30T10:01:00Z ]]

	run --separate-stderr jouet audit --until tomorrowish
	assert_failure 2
	assert_equal "$stderr" "jouet: --until: \`tomorrowish\` is not a time; write it as 7d, 12h, 2026-10-01 or RFC 3339"
}

# One session of jouet's own costs: a review-threads denial and a stop block
# in the ledger, a usage error after a `cd`, and a test log read right after
# `jouet test`.
own_session() {
	local id=99999999-2222-4333-8444-555555555555
	local dir="$HOME/.claude/projects/-work-own"
	mkdir -p "$dir" "$XDG_STATE_HOME/jouet/ledger"
	local at='"sessionId":"'$id'","cwd":"/work/own"'
	cat >"$dir/$id.jsonl" <<JSONL
{"type":"assistant","timestamp":"2026-11-01T10:00:00.000Z",$at,"message":{"id":"o1","usage":{"input_tokens":1000},"content":[{"type":"tool_use","id":"u1","name":"Bash","input":{"command":"jouet test --cmd -- cargo test"}}]}}
{"type":"user","timestamp":"2026-11-01T10:00:09.000Z",$at,"message":{"content":[{"type":"tool_result","tool_use_id":"u1","content":"cmd: failed (exit 101)\nfull log: /logs/test-1794000000000-42-0.log","is_error":true}]}}
{"type":"assistant","timestamp":"2026-11-01T10:00:10.000Z",$at,"message":{"id":"o2","usage":{"input_tokens":1000},"content":[{"type":"tool_use","id":"u2","name":"Bash","input":{"command":"tail -80 /logs/test-1794000000000-42-0.log"}}]}}
{"type":"user","timestamp":"2026-11-01T10:00:11.000Z",$at,"message":{"content":[{"type":"tool_result","tool_use_id":"u2","content":"thread panicked"}]}}
{"type":"assistant","timestamp":"2026-11-01T10:00:12.000Z",$at,"message":{"id":"o3","usage":{"input_tokens":1000},"content":[{"type":"tool_use","id":"u3","name":"Bash","input":{"command":"cd /work/own; jouet read src/a.rs --range 1,5"}}]}}
{"type":"user","timestamp":"2026-11-01T10:00:13.000Z",$at,"message":{"content":[{"type":"tool_result","tool_use_id":"u3","content":"Exit code 2\njouet: --range takes A:B","is_error":true}]}}
JSONL
	# 2026-11-01T10:00:00Z and the seconds after it.
	cat >"$XDG_STATE_HOME/jouet/ledger/ledger.jsonl" <<JSONL
{"ts":1793527200,"harness":"claude","session":"$id","event":"pre-tool-use","tool":"Bash","shape":"gh api <path>","decisions":[{"rule":"review-threads","action":"deny"}]}
{"ts":1793527201,"harness":"claude","session":"$id","event":"pre-tool-use","tool":"Bash","shape":"jouet pr threads <n>"}
{"ts":1793527202,"harness":"claude","session":"$id","event":"stop","decisions":[{"rule":"verify","action":"block"}]}
{"ts":1793527203,"harness":"claude","session":"$id","event":"post-tool-use","tool":"Bash","shape":"cargo test","exit_code":0,"decisions":[{"rule":"checks","action":"record:test"}]}
JSONL
}

@test "--jouet joins the ledger's blocks and denials with usage errors and test-log reads" {
	own_session
	run --separate-stderr jouet audit --jouet --display plain --project /work/own
	assert_success
	assert_equal "$stderr" ""
	assert_line --index 0 'jouet audit --jouet: 1 session in /work/own; jouet cost something in 1'
	assert_line '  stop blocks: 1 (1 resolved by a passing check before the next stop)'
	assert_line '    review-threads: 1 denied, 0 warned; then a jouet command 1, something else 0, nothing 0'
	assert_line --partial '  usage errors: 1, crashes: 0'
	assert_line '  test logs read right after `jouet test`: 1 of 1 run'
	assert_line '  99999999 claude /work/own: 1 stop block (1 resolved), review-threads denied ×1'
	assert_line '    usage `jouet read`: exit 2'
	assert_line '    test log read 1 request after `jouet test --cmd -- <v>…`: `tail -<n> <path>`'

	run --separate-stderr jouet audit --jouet --json --display plain --project /work/own --since 2026-11-01T10:00:00Z
	assert_success
	[[ "$(jq -c '.totals | [.stop_blocks, .stop_blocks_resolved, .denials, .usage_errors, .test_runs, .log_reads]' <<<"$output")" == '[1,1,1,1,1,1]' ]]
	[[ "$(jq -r '.sessions[0].rules[0].rule' <<<"$output")" == review-threads ]]
	[[ "$output" != *$'\n'* ]]

	# The window ends before the stop block, the log read and the usage error.
	run --separate-stderr jouet audit --jouet --json --display plain --project /work/own --until 2026-11-01T10:00:02Z
	assert_success
	[[ "$(jq -c '.totals | [.stop_blocks // 0, .denials, .usage_errors // 0, .test_runs, .log_reads // 0]' <<<"$output")" == '[0,1,0,1,0]' ]]
}

@test "carried charges each request's input to what put it in the context" {
	run --separate-stderr jouet audit --display plain --harness claude
	assert_success
	assert_line --partial "  carried: each request's input, charged to what put it in the context"
	assert_line --regexp '^  start of the session or after a compaction [0-9.]+%$'
	refute_output --partial 'files that kept costing most'

	run --separate-stderr jouet audit --json --display plain --harness claude --examples
	assert_success
	[[ "$(jq -r '.harnesses[0].carried.files[0].file' <<<"$output")" == src/lib.rs ]]
}

@test "--what-if replays denials exactly and a window as a range" {
	run --separate-stderr jouet audit --json --display plain --harness claude --what-if deny=Workflow,Nope
	assert_success
	[[ "$(jq -c '.harnesses[0].what_if | [.denied[0].name, .denied[0].input_tokens, .not_found[0], (.scenarios | length)]' <<<"$output")" == '["Workflow",259,"Nope",1]' ]]

	run --separate-stderr jouet audit --display plain --harness claude --what-if window=20k
	assert_success
	assert_line --partial '  what if (window 20k): replaying '
	assert_line --regexp '^  input .* to .* \([-+][0-9.]+% to [-+][0-9.]+%\), price-weighted .* compactions added$'

	run --separate-stderr jouet audit --what-if window=lots
	assert_failure 2
	assert_equal "$stderr" 'jouet: --what-if: window=`lots` is not a size; write it as 200k, 1m or 200000'
}

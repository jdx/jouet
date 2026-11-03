#!/usr/bin/env bats

# `jouet stats` over the fixture ledger in crates/jouet-audit/tests/fixtures,
# with the fixture Claude Code and Codex transcripts as the history before
# setup. That history is far too thin to estimate from, so every estimate
# here is refused with its reason.

setup() {
	load "test_helper/common_setup"
	_common_setup
	cp -R "$PROJECT_ROOT/crates/jouet-audit/tests/fixtures/home/." "$HOME/"
	mv "$HOME/claude" "$HOME/.claude"
	mv "$HOME/codex" "$HOME/.codex"
	export XDG_STATE_HOME="$HOME/state"
	export XDG_CONFIG_HOME="$HOME/.config"
	unset CLAUDE_CONFIG_DIR CODEX_HOME JOUET_STATS_SUMMARY
	stamp_revisions
}

# The fixture ledger's rule decisions, as from the revisions loaded now:
# `jouet stats` counts a rule only from its current revision. A line that is
# not JSON stays as it is.
stamp_revisions() {
	local revisions
	revisions=$(jouet rule list --json | jq -c 'reduce .rules[] as $r ({}; .[$r.name] = $r.revision)')
	for file in "$XDG_STATE_HOME"/jouet/ledger/*.jsonl; do
		jq -Rr --argjson rev "$revisions" '. as $line
			| (fromjson? | if has("decisions") then .decisions |= map(if .action | test("^(deny|warn|override)$") then .revision = $rev[.rule] else . end) else . end
				| tojson) // $line' "$file" >"$file.new"
		mv "$file.new" "$file"
	done
}

@test "measured figures come from the ledger, and thin history is not estimated" {
	run --separate-stderr jouet stats --display plain

	assert_success
	assert_equal "$stderr" ""
	assert_line --index 0 'jouet stats: 14 ledger entries in 2 sessions, 2026-10-31T00:00:00Z to 2026-10-31T00:07:00Z'
	assert_line --regexp '^  jouet wait ci +claude +1 +1 +0 +0 +4m$'
	assert_line --regexp '^  sleep-poll +1 +0 +1 +0 +0 +1 +0 +0$'
	assert_line --regexp '^  python-edit +1 +0 +1 +0 +1 +0 +1 +0$'
	assert_line '  checks recorded: test 1 (1 failed)'
	assert_line "  failing CI logs returned as the failing step's excerpt: 1"
	assert_line '  - jouet wait ci/run/pr: 1 run, not estimated: only 1 CI wait before setup, and an estimate needs 20'
	refute_output --partial 'estimated total'
	[[ -f "$XDG_STATE_HOME/jouet/stats/baselines.json" ]]
}

@test "rules are counted from the revision loaded now" {
	# One of python-edit's decisions from an older revision, and one from a
	# rule that is no longer loaded.
	ledger="$XDG_STATE_HOME/jouet/ledger/ledger.jsonl"
	jq -Rr '. as $line | (fromjson? | if .decisions[0]?.action == "override" then .decisions[0].revision = "0ld" else . end | tojson) // $line' \
		"$ledger" >"$ledger.new"
	mv "$ledger.new" "$ledger"
	jq -cn '{ts: 1793405230, harness: "claude", session: "s1", event: "pre-tool-use", tool: "Bash", shape: "git push",
		decisions: [{rule: "push-untested", action: "warn", revision: "123456789abc"}]}' >>"$ledger"
	# A rule of yours named like a built-in one loads broken; the built-in
	# one still decides, and is counted.
	mkdir -p "$XDG_CONFIG_HOME/jouet/rules"
	printf '[[rule]]\nname = "sleep-poll"\nwhy = "a copy"\nmessage = "no"\n[rule.match]\ncommand = "sleep"\n' \
		>"$XDG_CONFIG_HOME/jouet/rules/sleep-poll.toml"

	run --separate-stderr jouet stats --display plain
	assert_success
	assert_line --regexp '^  python-edit +1 +0 +1 +0 +0 +0 +1 +0$'
	assert_line --regexp '^  sleep-poll +1 +0 +1 +0 +0 +1 +0 +0$'
	refute_line --partial 'push-untested '
	assert_line '  rules are counted from the revision loaded now; left out: 1 decision of an older revision of a rule, or from before the ledger recorded revisions; and the rules not loaded here: push-untested'
	# What the warning cost still counts.
	assert_line --partial '  - rule warnings: 1, not estimated: '

	run --separate-stderr jouet stats --json --display plain
	[[ "$(jq -c '.measured | [.older_decisions, .rules_not_loaded]' <<<"$output")" == '[1,["push-untested"]]' ]]
}

@test "costs are shown beside savings, and no net is claimed when one cannot be priced" {
	run --separate-stderr jouet stats --display plain

	assert_success
	assert_line 'estimated savings: what these runs would have cost at your rates before setup (assumed, not measured):'
	assert_line 'estimated costs, priced the same way:'
	assert_line '  - calls a rule denied: 2, not estimated: only 8 requests with token counts before setup, and a price needs 20'
	assert_line --regexp '^  - requests carrying the session-start primer: 0, about 0 input tokens$'
	assert_line --partial '  - rule warnings: 1, not estimated: '
	assert_line 'estimated savings 0, costs 0: no net, since a cost above could not be priced.'

	run --separate-stderr jouet stats --json --display plain
	assert_success
	[[ "$(jq -r '[.costs[] | select(.harness == "claude") | .kind] | join(",")' <<<"$output")" == stop_blocks,denials,primer ]]
	[[ "$(jq -r '.after_setup | map(.harness) | join(",")' <<<"$output")" == claude,codex ]]
	[[ "$(jq 'has("net_input_tokens")' <<<"$output")" == false ]]
}

@test "a jouet command in a session records its own exit code, and the stats use it" {
	# s1 is a fixture session the hooks saw, from Claude Code.
	run --separate-stderr env CLAUDE_CODE_SESSION_ID=s1 jouet edit replace missing.txt --old a --new b
	assert_failure
	run --separate-stderr env CLAUDE_CODE_SESSION_ID=s1 jouet edit replace missing.txt --old a --new b --json
	assert_failure

	run --separate-stderr jq -c 'select(.event == "jouet-run") | [.harness, .shape, .exit_code != 0]' "$XDG_STATE_HOME/jouet/ledger/ledger.jsonl"
	assert_output '["jouet","jouet edit replace",true]
["jouet","jouet edit replace",true]'

	run --separate-stderr jouet stats --json --display plain
	assert_success
	[[ "$(jq -c '.measured.commands[] | select(.command == "jouet edit replace") | [.harness, .runs // 0, .own_runs, .own_failed]' <<<"$output")" == '["claude",0,2,2]' ]]
	[[ "$(jq '[.measured.overhead[] | .failed_runs // 0] | add' <<<"$output")" == 0 ]]

	# Outside a session, nothing is recorded.
	run --separate-stderr jouet edit replace missing.txt --old a --new b
	[[ "$(grep -c jouet-run "$XDG_STATE_HOME/jouet/ledger/ledger.jsonl")" == 2 ]]
}

@test "jouet pr and jouet run record their own exit codes too" {
	run --separate-stderr env CLAUDE_CODE_SESSION_ID=s1 JOUET_GITHUB_API_URL=http://127.0.0.1:9 jouet pr status 1 --repo a/b
	assert_failure
	local code=$status
	run --separate-stderr jq -c 'select(.event == "jouet-run") | [.shape, .exit_code]' "$XDG_STATE_HOME/jouet/ledger/ledger.jsonl"
	assert_output "[\"jouet pr status\",$code]"
}

@test "saved baselines are reused until --rescan" {
	run --separate-stderr jouet stats --json --display plain
	assert_success
	[[ "$(jq '.scan.files' <<<"$output")" -gt 0 ]]

	run --separate-stderr jouet stats --json --display plain
	assert_success
	[[ "$(jq '.scan.files' <<<"$output")" == 0 ]]
	[[ "$(jq -r '.baselines | map(.harness) | join(",")' <<<"$output")" == claude,codex ]]

	run --separate-stderr jouet stats --json --display plain --rescan
	assert_success
	[[ "$(jq '.scan.files' <<<"$output")" -gt 0 ]]
}

@test "--json keeps measured and estimated apart, empty fields left out" {
	run --separate-stderr jouet stats --json --display plain

	assert_success
	[[ "$(jq '.ledger.present' <<<"$output")" == true ]]
	[[ "$(jq '.measured.commands | length' <<<"$output")" == 4 ]]
	[[ "$(jq '[.estimates[] | has("refused")] | all' <<<"$output")" == true ]]
	[[ "$(jq 'has("estimated_input_tokens")' <<<"$output")" == false ]]
	[[ "$(jq -r '.baselines | map(.harness) | join(",")' <<<"$output")" == claude,codex ]]
}

@test "--since, --harness and --session narrow what is counted" {
	run --separate-stderr jouet stats --json --display plain --since 2026-10-31T00:06:00Z
	assert_success
	[[ "$(jq '.measured.entries' <<<"$output")" == 4 ]]

	run --separate-stderr jouet stats --json --display plain --harness codex
	assert_success
	[[ "$(jq -r '.measured.commands | map(.harness) | unique | join(",")' <<<"$output")" == codex ]]

	run --separate-stderr jouet stats --json --display plain --session c1
	assert_success
	[[ "$(jq '.measured.sessions' <<<"$output")" == 1 ]]
}

@test "--summary prints one line, in the configured style, or nothing" {
	run --separate-stderr jouet stats --summary
	assert_success
	assert_output --regexp '^jouet\[savings\]: '
	[[ "${#lines[@]}" == 1 ]]

	mkdir -p "$XDG_CONFIG_HOME/jouet"
	printf '[stats]\nsummary = "plain"\n' >"$XDG_CONFIG_HOME/jouet/config.toml"
	run --separate-stderr jouet stats --summary
	assert_success
	assert_output --regexp "^jouet\[savings\]: (rules steered 1 command to jouet|1 failing CI log returned as the failing step's excerpt|blocked finishing after a failed check once)$"

	run --separate-stderr env JOUET_STATS_SUMMARY=off jouet stats --summary
	assert_success
	assert_output ''

	# A value jouet cannot read counts as the default, so a typo never
	# breaks the stop hook.
	run --separate-stderr env JOUET_STATS_SUMMARY=loud jouet stats --summary
	assert_success
	assert_output --regexp '^jouet\[savings\]: '
}

@test "without a ledger it says so and prices one run of each command" {
	rm -rf "$XDG_STATE_HOME/jouet/ledger"

	run --separate-stderr jouet stats --display plain
	assert_success
	assert_line --index 0 "jouet stats: no ledger at $XDG_STATE_HOME/jouet/ledger, so nothing is measured yet. \`jouet setup\` starts one."
	assert_line 'what one jouet run would save, by your history:'
	assert_line '  - jouet wait ci/run/pr: not estimated: only 1 CI wait before setup, and an estimate needs 20'

	run --separate-stderr jouet stats --summary
	assert_success
	assert_output ''
}

@test "usage errors exit 2" {
	run --separate-stderr jouet stats --harness cursor
	assert_failure 2

	run --separate-stderr jouet stats --since soon --json
	assert_failure 2
	[[ "$(jq -r '.error.code' <<<"$output")" == usage ]]
}

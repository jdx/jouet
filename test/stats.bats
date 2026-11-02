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
}

@test "measured figures come from the ledger, and thin history is not estimated" {
	run --separate-stderr jouet stats --display plain

	assert_success
	assert_equal "$stderr" ""
	assert_line --index 0 'jouet stats: 14 ledger entries in 2 sessions, 2026-10-31T00:00:00Z to 2026-10-31T00:07:00Z'
	assert_line --regexp '^  jouet wait ci +claude +1 +1 +4m$'
	assert_line --regexp '^  sleep-poll +1 +0 +0 +1 +0 +0$'
	assert_line --regexp '^  python-edit +1 +0 +1 +0 +1 +0$'
	assert_line '  checks recorded: test 1 (1 failed)'
	assert_line "  failing CI logs returned as the failing step's excerpt: 1"
	assert_line '  - jouet wait ci/run/pr: 1 run, not estimated: only 1 CI wait before setup, and an estimate needs 20'
	refute_output --partial 'estimated total'
	[[ -f "$XDG_STATE_HOME/jouet/stats/baselines.json" ]]
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

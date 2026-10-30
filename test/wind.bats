#!/usr/bin/env bats

# Self-winding: `jouet wind` drafting rules from the hook ledger, `jouet rule
# new|promote|log` and `rule test --replay`, and `jouet unwind`. Each test
# starts from test/fixtures/ledger.jsonl: two sessions in which `mytool sync
# --all` keeps failing with 127, agents sleep a lot, and a user rule
# `no-tags` keeps getting overridden.

setup() {
	load "test_helper/common_setup"
	_common_setup
	unset XDG_CONFIG_HOME XDG_CACHE_HOME JOUET_RULES_DISABLE JOUET_WIND JOUET_WIND_AUTO_PROMOTE
	unset JOUET_SESSION_ID CLAUDE_SESSION_ID CODEX_THREAD_ID
	RULES="$HOME/.config/jouet/rules"
	LOG="$XDG_STATE_HOME/jouet/rule-log.jsonl"
	mkdir -p "$RULES" "$XDG_STATE_HOME/jouet/ledger"
	cp "$PROJECT_ROOT/test/fixtures/ledger.jsonl" "$XDG_STATE_HOME/jouet/ledger/ledger.jsonl"
}

# The session-start hook's primer for a session.
primer() {
	printf '{"hook_event_name":"SessionStart","session_id":"%s","cwd":"%s"}' "$1" "$PWD" |
		jouet hook claude session-start |
		jq -r '.hookSpecificOutput.additionalContext'
}

no_tags_rule() {
	cat >"$RULES/no-tags.luau" <<'LUAU'
--- why: tags are cut by the release job
return {
	check = function(call)
		for _, c in call.commands do
			if c.argv[1] == "git" and c.argv[2] == "tag" then
				return jouet.deny("Leave tags to the release job.")
			end
		end
		return nil
	end,
}
LUAU
}

@test "wind drafts a rule for each costly pattern and reads the ledger once" {
	run --separate-stderr jouet wind

	assert_success
	assert_line 'read 60 ledger entries; drafted n1, n2'
	assert_line --regexp '^n1  wind-mytool-sync-fails  .mytool sync --all. failed 4 times in 2 sessions \(last exit 127, not found\)'
	assert_line --regexp '^n2  wind-sleep  Agents spent 4m30s waiting in 6 long sleeps'
	# A test run that fails is ordinary work, not a pattern.
	refute_output --partial 'cargo'

	run --separate-stderr jouet wind --json
	run jq -c '[.read, (.pending | map(.id))]' <<<"$output"
	assert_output '[0,["n1","n2"]]'

	run --separate-stderr jouet wind show n1
	assert_success
	assert_line '--- mode: warn'
	assert_line '--- scope: user'
	assert_line --regexp 'mytool pull. worked instead'

	run --separate-stderr jouet wind show n9
	assert_failure 3
}

@test "a session start shows at most two notes, and each lapses" {
	jouet wind >/dev/null
	# A third pattern, a tool that is not installed, waits for tomorrow: at
	# most two notes are drafted a day.
	for s in s3 s3 s4; do
		printf '{"ts":1793400000,"harness":"claude","session":"%s","event":"post-tool-use","tool":"Bash","shape":"nope-cli run","exit_code":127}\n' "$s" >>"$XDG_STATE_HOME/jouet/ledger/ledger.jsonl"
	done
	run --separate-stderr jouet wind
	assert_line 'read 3 ledger entries'

	run primer sess-1
	assert_success
	assert_equal "$(grep -c '^jouet wind noticed:' <<<"$output")" 2
	assert_line --partial 'jouet rule new wind-mytool-sync-fails --from-note n1 --author sess-1'

	run --separate-stderr jouet wind dismiss n2
	assert_success
	run primer sess-2
	assert_equal "$(grep -c '^jouet wind noticed:' <<<"$output")" 1
	# Shown to a third session, n1 lapses.
	primer sess-3 >/dev/null
	run primer sess-4
	refute_output --partial 'jouet wind noticed'
}

@test "JOUET_WIND=0 turns winding off" {
	run --separate-stderr env JOUET_WIND=0 jouet wind
	assert_failure 4
	[[ "$stderr" == "jouet: winding is off; turn it on with "*"[wind] enabled = true"*" or unset JOUET_WIND" ]]

	export JOUET_WIND=0
	run primer sess-1
	assert_success
	refute_output --partial 'jouet wind noticed'
	[[ ! -e "$XDG_STATE_HOME/jouet/wind.json" ]]
}

@test "an adopted note becomes a warn-mode rule that passes its examples and replay" {
	jouet wind >/dev/null

	run --separate-stderr jouet rule new wind-mytool-sync-fails --from-note n1 --author sess-7
	assert_success
	assert_output --partial '(warn mode)'
	assert_line --partial 'jouet rule test wind-mytool-sync-fails --replay'

	run --separate-stderr jouet rule list
	assert_line --regexp '^wind-mytool-sync-fails +enabled +warn +user +sess-7 +[0-9-]+ +.mytool sync --all. failed'

	run --separate-stderr jouet rule test wind-mytool-sync-fails
	assert_success
	assert_line --regexp '^wind-mytool-sync-fails: ([0-9]+) of \1 examples passed$'

	run --separate-stderr jouet rule test wind-mytool-sync-fails --replay
	assert_success
	assert_line 'wind-mytool-sync-fails: fired on 4 of 28 calls in the ledger'
	assert_line --regexp '^ +4x +warn +mytool sync --all$'
	assert_line 'stays quiet on (near misses):'
	assert_line --regexp '^ +2x +allow +mytool pull$'

	# An adopted note is not shown again.
	run primer sess-8
	refute_output --partial 'from-note n1'
}

@test "replay fails a rule that fires on nothing the hooks have seen" {
	cat >"$RULES/no-rm.luau" <<'LUAU'
--- why: rm deletes for good
return { check = function(call) return if call.commands[1].program == "rm" then jouet.deny("no") else nil end }
LUAU
	run --separate-stderr jouet rule test no-rm --replay
	assert_failure 1
	assert_line 'no-rm: fired on 0 of 28 calls in the ledger'
	assert_line --partial 'it fired on nothing'

	rm "$XDG_STATE_HOME/jouet/ledger/ledger.jsonl"
	run --separate-stderr jouet rule test no-rm --replay
	assert_failure 4
	[[ "$stderr" == *'is empty'* ]]
}

@test "rule new writes a template, and refuses built-in names and clobbering" {
	run --separate-stderr env JOUET_SESSION_ID=sess-3 jouet rule new no-rm --why 'rm deletes for good'
	assert_success
	run head -7 "$RULES/no-rm.luau"
	assert_line '--- name: no-rm'
	assert_line '--- why: rm deletes for good'
	assert_line '--- mode: warn'
	assert_line '--- scope: user'
	assert_line '--- author: sess-3'

	run --separate-stderr jouet rule new no-rm --why again
	assert_failure 4
	run --separate-stderr jouet rule new git-stash --why 'allow stashing'
	assert_failure 4
	[[ "$stderr" == *'built-in rule'* ]]
	run --separate-stderr jouet rule new Bad_Name --why x
	assert_failure 2
	run --separate-stderr jouet rule new other
	assert_failure 2
	run --separate-stderr jouet rule new other --from-note n42
	assert_failure 3

	run jq -r '[.rule, .change, .by] | join(" ")' "$LOG"
	assert_output 'no-rm created sess-3'
}

@test "promote moves only a rule of yours to deny, and the log says why" {
	no_tags_rule
	run --separate-stderr jouet rule promote no-tags --reason 'it is right'
	assert_success
	assert_output 'no-tags now denies (was warn)'
	run --separate-stderr jouet rule test no-tags --command 'git tag v1'
	assert_output 'deny: Leave tags to the release job.'

	run --separate-stderr jouet rule promote no-tags
	assert_failure 4
	run --separate-stderr jouet rule promote git-stash
	assert_failure 4
	[[ "$stderr" == *'builtin rule'* ]]
	run --separate-stderr jouet rule promote nope
	assert_failure 3

	run --separate-stderr jouet rule log
	assert_line --regexp '^[0-9-]+  no-tags  promoted warn -> deny  by user: it is right$'
	run --separate-stderr jouet rule log other
	assert_output 'no changes logged'
}

@test "unwind proposes an overridden rule of yours, never a built-in one, and retires it" {
	no_tags_rule
	run --separate-stderr jouet unwind
	assert_success
	assert_line 'propose  no-tags  4 of its 6 firings were overridden: the command went ahead anyway'
	refute_output --partial 'git-stash'
	[[ -f "$RULES/no-tags.luau" ]]

	run --separate-stderr jouet unwind --apply
	assert_success
	assert_line --partial 'retired  no-tags'
	[[ ! -e "$RULES/no-tags.luau" ]]
	[[ -f "$RULES/retired/no-tags.luau" ]]
	run jq -r '[.rule, .change] | join(" ")' "$LOG"
	assert_output 'no-tags retired'

	run --separate-stderr jouet unwind
	assert_output --partial 'nothing to unwind'
}

@test "auto-promotion passes over a rule that gets overridden" {
	no_tags_rule
	run --separate-stderr env JOUET_WIND_AUTO_PROMOTE=1 JOUET_WIND_PROMOTE_AFTER=2 jouet wind
	# Overridden, so it is not promoted even when promotion is on.
	refute_output --partial 'promoted'
	run --separate-stderr jouet rule list
	assert_line --regexp '^no-tags +enabled +warn'
}

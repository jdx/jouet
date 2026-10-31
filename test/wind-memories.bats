#!/usr/bin/env bats

# Memory upkeep: `jouet wind memories` reads the agents' saved memories and
# notices the stale ones, the session-start hook shows them in their
# project, and `jouet wind review` lists them for `memory_update` and
# `memory_delete` proposals. The memories here name no PR, so nothing asks
# GitHub; tests/wind_memories.rs covers that against a fake.

CLAUDE_ID=d1d1d1d1-0000-4000-8000-000000000001

setup() {
	load "test_helper/common_setup"
	_common_setup
	unset XDG_CONFIG_HOME XDG_CACHE_HOME JOUET_WIND JOUET_WIND_MEMORIES JOUET_SHARE_CONTRIBUTE
	unset CLAUDE_CONFIG_DIR CODEX_HOME JOUET_HOOKS JOUET_HOOKS_PRIME
	export JOUET_GITHUB_API_URL=http://127.0.0.1:9 JOUET_GITHUB_TOKEN=unused
	MEMORY="$HOME/.claude/projects/-work-app/memory"
	mkdir -p "$MEMORY"
	printf -- '- [old](old.md)\n- [stash](stash.md)\n' >"$MEMORY/MEMORY.md"
	cat >"$MEMORY/old.md" <<'MD'
---
name: old-command
description: how to start
---

Run `jouet frobnicate --now` before anything else.
MD
	cat >"$MEMORY/stash.md" <<'MD'
---
name: stash-anyway
description: the stash rule
---

The `git-stash` rule denies wrongly in this repo, so override it.
MD
	printf '# Global\n\nKeep PR titles short.\n' >"$HOME/.claude/CLAUDE.md"
}

# The session-start hook's added context for SESSION, as if run in CWD.
primer() {
	jq -cn --arg s "$1" --arg cwd "$2" '{hook_event_name: "SessionStart", session_id: $s, cwd: $cwd}' |
		jouet hook claude session-start |
		jq -r '.hookSpecificOutput.additionalContext'
}

@test "memories lists what looks stale, and never changes a memory" {
	local before
	before=$(cat "$MEMORY"/*.md)

	run --separate-stderr jouet wind memories

	assert_success
	assert_line 'read 3 memories (1 projects); asked GitHub about 0 references'
	assert_line 'old-command  ~/.claude/projects/-work-app/memory/old.md'
	assert_line "  names \`jouet frobnicate\`, which this jouet does not have"
	assert_line "  seems to work around jouet's built-in rule \`git-stash\`"
	assert_equal "$(cat "$MEMORY"/*.md)" "$before"

	# Without --record nothing is noted.
	run --separate-stderr jouet wind --json
	run jq -r '.memories // [] | length' <<<"$output"
	assert_output 0
}

@test "recorded notes show at session starts in the memory's project, until dismissed" {
	run --separate-stderr jouet wind memories --record --json
	assert_success
	run jq -c '.recorded' <<<"$output"
	assert_output '["m1","m2"]'

	run primer s1 /work/app
	assert_line --partial "jouet wind: memory \`old-command\` names \`jouet frobnicate\`, which this jouet does not have: it may be out of date."
	assert_line --partial "jouet wind: memory \`stash-anyway\` seems to work around jouet's built-in rule \`git-stash\`: a jouet bug belongs with jouet's maintainers, not in a memory."

	# Another project's sessions never see them.
	run primer s2 /elsewhere
	refute_output --partial 'jouet wind: memory'

	run --separate-stderr jouet wind show m1
	assert_success
	assert_line 'path: ~/.claude/projects/-work-app/memory/old.md'

	run --separate-stderr jouet wind dismiss m1
	assert_success
	run --separate-stderr jouet wind adopt m2
	assert_success
	run primer s3 /work/app
	refute_output --partial 'jouet wind: memory'

	# Found again, a dismissed note is not recorded again.
	run --separate-stderr jouet wind memories --record --json
	run jq -c '.recorded // []' <<<"$output"
	assert_output '[]'
}

@test "with contribute on, a memory working around jouet becomes an idea to share" {
	mkdir -p "$HOME/.config/jouet"
	printf '[share]\ncontribute = true\n' >"$HOME/.config/jouet/config.toml"

	run --separate-stderr jouet wind memories --record --json
	assert_success
	run jq -r '.recorded | length' <<<"$output"
	assert_output 2

	run --separate-stderr jouet wind show m2 --json
	run jq -r '.idea' <<<"$output"
	assert_output --regexp '^note-a-saved-memory-works-around'

	run --separate-stderr jouet share --list
	assert_success
	assert_output --partial "An agent saved a memory to get past \`git-stash\`, so the rule may misfire: \"The \`git-stash\` rule denies wrongly in this repo, so override it.\""
}

@test "memory upkeep is off with JOUET_WIND_MEMORIES=0" {
	JOUET_WIND_MEMORIES=0 run --separate-stderr jouet wind memories
	assert_failure 4
	assert_equal "$stderr" "jouet: memory upkeep is off; turn it on with \`[wind] memories = true\` (and \`enabled\`), or unset JOUET_WIND_MEMORIES and JOUET_WIND"
}

@test "a review lists the project's memories, and propose checks the target" {
	local fixtures="$PROJECT_ROOT/crates/jouet-audit/tests/fixtures/digest"
	cp -r "$fixtures/claude/projects/-work-app/." "$HOME/.claude/projects/-work-app/"
	touch "$HOME/.claude/projects/-work-app/$CLAUDE_ID.jsonl"

	run --separate-stderr jouet wind review d1d1
	assert_success
	assert_line '## Memories'
	assert_line '- ~/.claude/projects/-work-app/memory/old.md — old-command: how to start'
	assert_line --partial '- ~/.claude/CLAUDE.md — ~/.claude/CLAUDE.md: Global'
	assert_output --partial '"memory_delete"'

	run --separate-stderr jouet wind propose <<'JSON'
{"session": "d1d1d1d1", "proposals": [
  {"kind": "memory_delete", "title": "Drop the frobnicate step", "why": "jouet has no frobnicate (L4).", "evidence": ["L4"], "target": "~/.claude/projects/-work-app/memory/old.md", "draft": "The command is gone."},
  {"kind": "memory_update", "title": "Fix a memory elsewhere", "why": "x", "target": "~/nowhere.md", "draft": "x"}
]}
JSON
	assert_success
	assert_line 'recorded p1; the next session starts show it'
	assert_line "skipped \"Fix a memory elsewhere\": \`~/nowhere.md\` is not one of the memories the review listed"
}

@test "the stop hook starts the check in the background, once a day" {
	local stop='{"hook_event_name": "Stop", "session_id": "s1", "cwd": "/work/app"}'
	jouet hook claude stop <<<"$stop"
	run jq -r '.memories.last_check > 0' "$XDG_STATE_HOME/jouet/wind.json"
	assert_output true

	# The check records its notes on its own; give it a few seconds.
	for _ in $(seq 50); do
		[[ "$(jq -r '.memories.notes | length' "$XDG_STATE_HOME/jouet/wind.json")" == 2 ]] && break
		sleep 0.1
	done
	run jq -r '[.memories.notes[].id] | join(" ")' "$XDG_STATE_HOME/jouet/wind.json"
	assert_output 'm1 m2'

	# Not again the same day.
	local checked
	checked=$(jq -r '.memories.last_check' "$XDG_STATE_HOME/jouet/wind.json")
	jouet hook claude stop <<<"$stop"
	run jq -r '.memories.last_check' "$XDG_STATE_HOME/jouet/wind.json"
	assert_output "$checked"
}

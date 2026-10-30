#!/usr/bin/env bats

# `jouet setup`, `jouet skill` and `jouet agent-help` as a person runs them:
# HOME is a scratch directory, so every file lands under the test's tmpdir.

setup() {
	load "test_helper/common_setup"
	_common_setup
	unset CLAUDE_CONFIG_DIR CODEX_HOME
}

@test "setup --dry-run shows the change and writes nothing" {
	run --separate-stderr jouet setup claude --dry-run

	assert_success
	assert_output --partial "would create $HOME/.claude/settings.json"
	assert_output --partial '+            "command": "jouet hook claude pre-tool-use",'
	assert_output --partial "would create $HOME/.claude/skills/jouet/SKILL.md"
	assert [ ! -e "$HOME/.claude" ]
}

@test "setup merges into existing hooks, and a re-run changes nothing" {
	mkdir -p "$HOME/.claude"
	printf '%s\n' '{"model":"opus","hooks":{"Stop":[{"hooks":[{"type":"command","command":"other-tool stop"}]}]}}' \
		>"$HOME/.claude/settings.json"

	run --separate-stderr jouet setup claude
	assert_success
	assert_line "updated $HOME/.claude/settings.json"
	assert_line "created $HOME/.claude/skills/jouet/SKILL.md"
	run jq -c '[.model, [.hooks.Stop[].hooks[].command]]' "$HOME/.claude/settings.json"
	assert_output '["opus",["other-tool stop","jouet hook claude stop"]]'

	cp "$HOME/.claude/settings.json" before.json
	run --separate-stderr jouet setup claude
	assert_success
	assert_line "unchanged $HOME/.claude/settings.json"
	assert_line "unchanged $HOME/.claude/skills/jouet/SKILL.md"
	run cmp before.json "$HOME/.claude/settings.json"
	assert_success
}

@test "setup --uninstall removes only what jouet added" {
	mkdir -p "$HOME/.claude"
	printf '%s\n' '{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"other-tool stop"}]}]}}' \
		>"$HOME/.claude/settings.json"
	jouet setup claude >/dev/null

	run --separate-stderr jouet setup claude --uninstall

	assert_success
	assert_line "removed $HOME/.claude/skills/jouet/SKILL.md"
	run jq -c . "$HOME/.claude/settings.json"
	assert_output '{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"other-tool stop"}]}]}}'
}

@test "setup never replaces or removes a skill it did not write, or one edited since" {
	mkdir -p "$HOME/.claude/skills/jouet"
	printf 'my own notes\n' >"$HOME/.claude/skills/jouet/SKILL.md"

	run --separate-stderr jouet setup claude --uninstall
	assert_success
	assert_line "kept $HOME/.claude/skills/jouet/SKILL.md"
	assert_content "$HOME/.claude/skills/jouet/SKILL.md" $'my own notes\n'

	run --separate-stderr jouet setup claude
	assert_line "kept $HOME/.claude/skills/jouet/SKILL.md"
	assert_line --partial 'move it aside to install'
	assert_content "$HOME/.claude/skills/jouet/SKILL.md" $'my own notes\n'

	# jouet's own, once edited, is the user's.
	rm "$HOME/.claude/skills/jouet/SKILL.md"
	jouet setup claude >/dev/null
	printf 'one more rule\n' >>"$HOME/.claude/skills/jouet/SKILL.md"
	run --separate-stderr jouet setup claude --uninstall
	assert_line "kept $HOME/.claude/skills/jouet/SKILL.md"
}

@test "setup codex writes hooks.json and the skill under .agents, and says to trust the hooks" {
	run --separate-stderr jouet setup codex --json

	assert_success
	run jq -r '.files[].path, .notes[0]' <<<"$output"
	assert_line "$HOME/.codex/hooks.json"
	assert_line "$HOME/.agents/skills/jouet/SKILL.md"
	assert_line --partial "run /hooks in Codex"
	run jq -r '.hooks.PreToolUse[0].matcher' "$HOME/.codex/hooks.json"
	assert_output '^Bash$'
}

@test "setup --project writes into the repository root" {
	mkdir -p repo/.git repo/sub
	cd repo/sub

	run --separate-stderr jouet setup codex --project

	assert_success
	assert [ -f "$BATS_TEST_TMPDIR/work/repo/.codex/hooks.json" ]
	assert [ -f "$BATS_TEST_TMPDIR/work/repo/.agents/skills/jouet/SKILL.md" ]
}

@test "setup refuses a settings file that is not JSON, and leaves it alone" {
	mkdir -p "$HOME/.claude"
	printf '{ not json\n' >"$HOME/.claude/settings.json"

	run --separate-stderr jouet setup claude

	assert_failure 4
	assert_equal "$stderr" "jouet: $HOME/.claude/settings.json is not valid JSON (key must be a string at line 1 column 3); fix it or move it aside, then run this again"
	assert_content "$HOME/.claude/settings.json" $'{ not json\n'
}

@test "setup rejects an unknown harness and conflicting scopes" {
	run --separate-stderr jouet setup cursor
	assert_failure 2

	run --separate-stderr jouet setup claude --global --project
	assert_failure 2
}

@test "skill prints a thin SKILL.md, or an AGENTS.md snippet" {
	run --separate-stderr jouet skill
	assert_success
	assert_line 'name: jouet'
	assert_output --partial 'jouet agent-help'

	run --separate-stderr jouet skill --format agents-md
	assert_success
	assert_line --index 0 '## jouet'
}

@test "agent-help covers the tool in under 60 lines and mentions installed companions" {
	printf '#!/bin/sh\n' >"$BATS_TEST_TMPDIR/bin/bd"
	chmod +x "$BATS_TEST_TMPDIR/bin/bd"

	run --separate-stderr jouet agent-help

	assert_success
	assert [ "${#lines[@]}" -le 60 ]
	assert_output --partial 'jouet edit replace <PATH> --old TEXT --new TEXT'
	assert_output --partial 'Also installed: bd (beads)'
}

@test "agent-help for an unknown command exits not found" {
	run --separate-stderr jouet agent-help frobnicate
	assert_failure 3
}

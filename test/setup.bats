#!/usr/bin/env bats

# `jouet setup` as a person runs it: HOME is a scratch directory, so every
# file lands under the test's tmpdir.

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

@test "setup writes through a symlinked settings file and keeps its mode" {
	mkdir -p "$HOME/dotfiles" "$HOME/.claude"
	printf '%s\n' '{"model":"opus"}' >"$HOME/dotfiles/settings.json"
	chmod 644 "$HOME/dotfiles/settings.json"
	ln -s ../dotfiles/settings.json "$HOME/.claude/settings.json"

	run --separate-stderr jouet setup claude

	assert_success
	assert [ -L "$HOME/.claude/settings.json" ]
	run jq -r '.model, .hooks.Stop[0].hooks[0].command' "$HOME/dotfiles/settings.json"
	assert_output $'opus\njouet hook claude stop'
	run sh -c 'stat -c %a "$1" 2>/dev/null || stat -f %Lp "$1"' _ "$HOME/dotfiles/settings.json"
	assert_output 644
}

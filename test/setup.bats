#!/usr/bin/env bats

# `jouet setup` as a person runs it: HOME is a scratch directory, so every
# file lands under the test's tmpdir.

setup() {
	load "test_helper/common_setup"
	_common_setup
	unset CLAUDE_CONFIG_DIR CODEX_HOME
}

# Fails unless FILE matches test/fixtures/setup/NAME byte for byte.
assert_golden() {
	run diff -u "$PROJECT_ROOT/test/fixtures/setup/$2" "$1"
	assert_success
}

# Prints the command jouet installed for EVENT in settings FILE.
hook_command() {
	jq -r --arg e "$2" '.hooks[$e][] | .hooks[] | .command | select(test("jouet hook"))' "$1"
}

@test "setup --dry-run shows the change and writes nothing" {
	run --separate-stderr jouet setup claude --dry-run

	assert_success
	assert_output --partial "would create $HOME/.claude/settings.json"
	assert_output --partial '+            "command": "sh -c '"'"'command -v jouet >/dev/null 2>&1 || exit 0; exec jouet hook claude pre-tool-use'"'"'",'
	refute_output --partial "SKILL.md"
	assert [ ! -e "$HOME/.claude" ]
}

@test "setup leaves the skill to the primer unless asked, and removes its own from an earlier install" {
	run --separate-stderr jouet setup claude --skill
	assert_success
	assert_line "created $HOME/.claude/skills/jouet/SKILL.md"

	run --separate-stderr jouet setup claude
	assert_success
	assert_line "removed $HOME/.claude/skills/jouet/SKILL.md"
	assert [ ! -e "$HOME/.claude/skills/jouet" ]

	# Without the primer, the skill is what tells the agent about jouet.
	JOUET_HOOKS_PRIME=0 run --separate-stderr jouet setup claude
	assert_success
	assert_line "created $HOME/.claude/skills/jouet/SKILL.md"
}

@test "setup merges into existing hooks, and a re-run changes nothing" {
	mkdir -p "$HOME/.claude"
	printf '%s\n' '{"model":"opus","hooks":{"Stop":[{"hooks":[{"type":"command","command":"other-tool stop"}]}]}}' \
		>"$HOME/.claude/settings.json"

	run --separate-stderr jouet setup claude --skill
	assert_success
	assert_line "updated $HOME/.claude/settings.json"
	assert_line "created $HOME/.claude/skills/jouet/SKILL.md"
	run jq -c '[.model, [.hooks.Stop[].hooks[].command]]' "$HOME/.claude/settings.json"
	assert_output '["opus",["other-tool stop","sh -c '"'"'command -v jouet >/dev/null 2>&1 || exit 0; exec jouet hook claude stop'"'"'"]]'

	cp "$HOME/.claude/settings.json" before.json
	run --separate-stderr jouet setup claude --skill
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
	jouet setup claude --skill >/dev/null

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

	run --separate-stderr jouet setup claude --skill
	assert_line "kept $HOME/.claude/skills/jouet/SKILL.md"
	assert_line --partial 'move it aside to install'
	assert_content "$HOME/.claude/skills/jouet/SKILL.md" $'my own notes\n'

	# Without --skill, setup has nothing to say about it.
	run --separate-stderr jouet setup claude
	refute_output --partial "SKILL.md"
	assert_content "$HOME/.claude/skills/jouet/SKILL.md" $'my own notes\n'

	# jouet's own, once edited, is the user's.
	rm "$HOME/.claude/skills/jouet/SKILL.md"
	jouet setup claude --skill >/dev/null
	printf 'one more rule\n' >>"$HOME/.claude/skills/jouet/SKILL.md"
	run --separate-stderr jouet setup claude --uninstall
	assert_line "kept $HOME/.claude/skills/jouet/SKILL.md"
	run --separate-stderr jouet setup claude
	refute_output --partial "SKILL.md"
}

@test "setup codex writes hooks.json and the skill under .agents, and says to trust the hooks" {
	run --separate-stderr jouet setup codex --skill --json

	assert_success
	run jq -r '.files[].path, .notes[0]' <<<"$output"
	assert_line "$HOME/.codex/hooks.json"
	assert_line "$HOME/.agents/skills/jouet/SKILL.md"
	assert_line --partial "run /hooks in Codex"
	run jq -r '.hooks.PreToolUse[0].matcher' "$HOME/.codex/hooks.json"
	assert_output '^Bash$'
}

@test "setup --shared writes into the repository root" {
	mkdir -p repo/.git repo/sub
	cd repo/sub

	run --separate-stderr jouet setup codex --shared --skill

	assert_success
	assert_line --partial "everyone who clones this repository gets these hooks"
	assert_golden "$BATS_TEST_TMPDIR/work/repo/.codex/hooks.json" codex-hooks.json
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

	run --separate-stderr jouet setup claude --global --shared
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
	assert_output "opus
sh -c 'command -v jouet >/dev/null 2>&1 || exit 0; exec jouet hook claude stop'"
	run sh -c 'stat -c %a "$1" 2>/dev/null || stat -f %Lp "$1"' _ "$HOME/dotfiles/settings.json"
	assert_output 644
}

@test "setup --project writes your own settings.local.json at the repository root, and says when git would commit it" {
	init_repo
	work=$(pwd -P) # macOS reports /private/tmp for /tmp
	mkdir sub
	cd sub

	run --separate-stderr jouet setup claude --project --skill

	assert_success
	assert_line "created $work/.claude/settings.local.json"
	assert_line "created $work/.claude/skills/jouet/SKILL.md"
	assert_line "note: git does not ignore .claude/settings.local.json or .claude/skills/jouet/SKILL.md, so \`git add\` could commit them: list them in .gitignore, or in .git/info/exclude for this clone alone"
	assert_golden "$work/.claude/settings.local.json" claude-hooks.json
	assert [ ! -e "$work/.claude/settings.json" ]

	printf '.claude/settings.local.json\n.claude/skills/jouet/\n' >>"$work/.git/info/exclude"
	run --separate-stderr jouet setup claude --project --skill
	assert_success
	refute_line --partial "git does not ignore"
}

@test "setup --shared writes the tracked settings.json" {
	init_repo

	run --separate-stderr jouet setup claude --shared --json

	assert_success
	run jq -c '[.scope, .shared]' <<<"$output"
	assert_output '["project",true]'
	assert_golden .claude/settings.json claude-hooks.json
	assert [ ! -e .claude/settings.local.json ]
}

@test "setup --project from a linked worktree writes the main checkout's settings.local.json" {
	init_repo
	work=$(pwd -P) # macOS reports /private/tmp for /tmp
	commit_file README.md hi init
	git worktree add -q .claude/worktrees/w1 -b w1
	cd .claude/worktrees/w1

	run --separate-stderr jouet setup claude --project

	assert_success
	assert_line "created $work/.claude/settings.local.json"
	assert [ ! -e .claude/settings.local.json ]
}

@test "setup refuses --project for Codex, which has no hooks file of your own" {
	init_repo
	work=$(pwd -P) # macOS reports /private/tmp for /tmp

	run --separate-stderr jouet setup codex --project

	assert_failure 2
	assert_equal "$stderr" "jouet: Codex has no project hooks file of your own: .codex/hooks.json is the repository's, shared with everyone who clones it. Use --global, or --project --shared to install there anyway"
	assert [ ! -e .codex ]

	# Uninstalling still finds what an older `--project` put there.
	jouet setup codex --shared >/dev/null
	run --separate-stderr jouet setup codex --project --uninstall
	assert_success
	assert_line "updated $work/.codex/hooks.json"
	assert_content .codex/hooks.json $'{}\n'
}

@test "setup upgrades bare hooks from an older version without duplicating them" {
	mkdir -p "$HOME/.claude"
	jq '.hooks[][].hooks[].command |= (sub("^sh -c .*exec "; "") | rtrimstr("\u0027"))' \
		"$PROJECT_ROOT/test/fixtures/setup/claude-hooks.json" >"$HOME/.claude/settings.json"
	run hook_command "$HOME/.claude/settings.json" Stop
	assert_output "jouet hook claude stop"

	run --separate-stderr jouet setup claude
	assert_success
	assert_line "updated $HOME/.claude/settings.json"
	assert_golden "$HOME/.claude/settings.json" claude-hooks.json
}

@test "setup --uninstall removes jouet's hooks from either project file" {
	init_repo
	work=$(pwd -P) # macOS reports /private/tmp for /tmp
	jouet setup claude --shared --skill >/dev/null
	run --separate-stderr jouet setup claude --project --uninstall
	assert_success
	assert_line "updated $work/.claude/settings.json"
	assert_line "removed $work/.claude/skills/jouet/SKILL.md"
	assert_content .claude/settings.json $'{}\n'

	jouet setup claude --project >/dev/null
	run --separate-stderr jouet setup claude --shared --uninstall
	assert_success
	assert_line "updated $work/.claude/settings.local.json"
	assert_content .claude/settings.local.json $'{}\n'
}

@test "an installed hook runs jouet, and does nothing at all once jouet is not on PATH" {
	jouet setup claude >/dev/null
	command=$(hook_command "$HOME/.claude/settings.json" PreToolUse)
	input='{"session_id":"s1","hook_event_name":"PreToolUse","tool_name":"Bash","tool_input":{"command":"git add -A"},"cwd":"/tmp"}'

	run --separate-stderr sh -c "$command" <<<"$input"
	assert_success
	assert_output --partial '"permissionDecision":"deny"'

	# A PATH with a shell and nothing else.
	mkdir -p "$BATS_TEST_TMPDIR/sh-only"
	ln -s "$(command -v sh)" "$BATS_TEST_TMPDIR/sh-only/sh"
	run --separate-stderr env PATH="$BATS_TEST_TMPDIR/sh-only" sh -c "$command" <<<"$input"
	assert_success
	assert_output ""
	assert_equal "$stderr" ""
}

@test "setup says when jouet is not on PATH" {
	run --separate-stderr env PATH="/usr/bin:/bin" "$JOUET_BIN" setup claude --dry-run
	assert_success
	assert_line "note: jouet is not on PATH here, so the hooks will do nothing until it is"
}

@test "setup recommends fnox proxy run in a repository that uses fnox" {
	git init -q repo
	printf '[secrets]\n' >repo/fnox.toml
	cd repo

	run --separate-stderr jouet setup claude --project --dry-run

	assert_success
	assert_output --partial "note: this repository uses fnox: start the agent with \`fnox proxy run -- claude\`"
}

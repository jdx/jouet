#!/usr/bin/env bats

# `jouet setup` as a person runs it: HOME is a scratch directory, so every
# file lands under the test's tmpdir.

setup() {
	load "test_helper/common_setup"
	_common_setup
	unset CLAUDE_CONFIG_DIR CODEX_HOME
}

@test "setup can print the benchmark handoff without prompting" {
	run --separate-stderr jouet setup codex --benchmark
	assert_success
	assert_output --partial 'Start a new agent session in the project'
	assert_output --partial 'Help me compare this project with and without jouet.'
	assert_output --partial 'Remove private project details and secrets from the draft'
	assert_output --partial 'approve it before posting anything'
	refute [ -s "$HOME/.codex/auth.json" ]
}

@test "JSON setup includes the benchmark prompt only when requested" {
	run --separate-stderr jouet setup codex --benchmark --json
	assert_success
	run jq -r '.benchmark_prompt' <<<"$output"
	assert_success
	assert_output --partial 'Help me compare this project with and without jouet.'
	assert_output --partial 'Remove private project details and secrets from the draft'
	run --separate-stderr jouet setup codex --json
	assert_success
	run jq 'has("benchmark_prompt")' <<<"$output"
	assert_output false
}

@test "interactive setup offers a benchmark handoff" {
	run in_tty jouet setup codex
	assert_success
	assert_output --partial 'Would you like a prompt to compare this project'
}

@test "a benchmark handoff cannot be combined with uninstall or dry run" {
	run --separate-stderr jouet setup codex --benchmark --uninstall
	assert_failure 2
	run --separate-stderr jouet setup codex --benchmark --dry-run
	assert_failure 2
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
	assert_output --partial '+            "command": "sh -c '"'"'jouet --version >/dev/null 2>&1 || exit 0; exec jouet hook claude pre-tool-use'"'"'",'
	refute_output --partial jouet-init
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

@test "setup installs onboarding only when asked, for every harness, and prints its invocation" {
	run --separate-stderr jouet setup claude
	assert_success
	refute_output --partial jouet-init
	assert [ ! -e "$HOME/.claude/skills/jouet-init" ]

	for harness in claude codex cursor gemini copilot; do
		run --separate-stderr jouet setup "$harness" --init-skill --json
		assert_success
		if [[ "$harness" == claude ]]; then
			skill_path="$HOME/.claude/skills/jouet-init/SKILL.md"
		else
			skill_path="$HOME/.agents/skills/jouet-init/SKILL.md"
		fi
		run jq -r --arg path "$skill_path" '.files[] | select(.path == $path) | .action' <<<"$output"
		assert_success
		assert [ -f "$skill_path" ]
		run cat "$skill_path"
		assert_line 'name: jouet-init'
	done

	run --separate-stderr jouet setup claude --init-skill
	assert_line --partial 'ask: /jouet-init'
	run --separate-stderr jouet setup codex --init-skill
	assert_line --partial "ask: \$jouet-init"

	# Without the flag, an earlier setup's unedited copy goes.
	run --separate-stderr jouet setup claude
	assert_success
	assert_line "removed $HOME/.claude/skills/jouet-init/SKILL.md"
	assert [ ! -e "$HOME/.claude/skills/jouet-init" ]
	refute_output --partial 'next:'
}

@test "onboarding is idempotent, uninstalled when unedited, and preserves local edits" {
	jouet setup codex --init-skill >/dev/null
	skill_path="$HOME/.agents/skills/jouet-init/SKILL.md"
	cp "$skill_path" before-skill
	run --separate-stderr jouet setup codex --init-skill
	assert_success
	assert_line "unchanged $skill_path"
	run cmp before-skill "$skill_path"
	assert_success

	run --separate-stderr jouet setup codex --uninstall
	assert_success
	assert_line "removed $skill_path"
	assert [ ! -e "$skill_path" ]
	refute_output --partial 'next:'

	jouet setup codex --init-skill >/dev/null
	printf '\nmy project notes\n' >>"$skill_path"
	cp "$skill_path" edited-skill
	run --separate-stderr jouet setup codex --init-skill
	assert_success
	assert_line "kept $skill_path"
	run --separate-stderr jouet setup codex --uninstall
	assert_success
	assert_line "kept $skill_path"
	run cmp edited-skill "$skill_path"
	assert_success
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
	assert_output '["opus",["other-tool stop","sh -c '"'"'jouet --version >/dev/null 2>&1 || exit 0; exec jouet hook claude stop'"'"'"]]'

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
	refute_output --partial "/skills/jouet/SKILL.md"
	assert_content "$HOME/.claude/skills/jouet/SKILL.md" $'my own notes\n'

	# jouet's own, once edited, is the user's.
	rm "$HOME/.claude/skills/jouet/SKILL.md"
	jouet setup claude --skill >/dev/null
	printf 'one more rule\n' >>"$HOME/.claude/skills/jouet/SKILL.md"
	run --separate-stderr jouet setup claude --uninstall
	assert_line "kept $HOME/.claude/skills/jouet/SKILL.md"
	run --separate-stderr jouet setup claude
	refute_output --partial "/skills/jouet/SKILL.md"
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
	run --separate-stderr jouet setup vim
	assert_failure 2
	assert_equal "$stderr" "jouet: the harness must be claude, codex, cursor, gemini or copilot, not \`vim\`"

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
sh -c 'jouet --version >/dev/null 2>&1 || exit 0; exec jouet hook claude stop'"
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

	printf '.claude/settings.local.json\n.claude/skills/jouet/\n.claude/skills/jouet-init/\n' >>"$work/.git/info/exclude"
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

@test "Claude Code's project skill links to the shared .agents copy, and uninstalling removes only the link" {
	init_repo
	jouet setup codex --shared --init-skill >/dev/null
	jouet setup claude --shared --init-skill >/dev/null
	assert [ -L .claude/skills/jouet-init ]
	assert_equal "$(readlink .claude/skills/jouet-init)" ../../.agents/skills/jouet-init
	assert [ -f .agents/skills/jouet-init/SKILL.md ]
	# Running it again changes nothing.
	run --separate-stderr jouet setup claude --shared --init-skill --json
	assert_success
	run jq -r '.files[] | select(.path | contains("jouet-init")) | .action' <<<"$output"
	# The shared copy and the link to it.
	assert_output $'unchanged\nunchanged'
	jouet setup claude --shared --init-skill --uninstall >/dev/null
	assert [ ! -e .claude/skills/jouet-init ]
	assert [ -f .agents/skills/jouet-init/SKILL.md ]
}

@test "uninstalling Claude Code's shared skill, with no other harness set up, removes the shared copy too" {
	init_repo
	jouet setup claude --shared --init-skill >/dev/null
	assert [ -L .claude/skills/jouet-init ]
	jouet setup claude --shared --init-skill --uninstall >/dev/null
	assert [ ! -e .claude/skills/jouet-init ]
	assert [ ! -e .agents/skills/jouet-init ]
}

@test "a dry run shows the shared copy and the link, and other harnesses keep the copy Claude Code links to" {
	init_repo
	work=$(pwd -P)
	run --separate-stderr jouet setup claude --shared --init-skill --dry-run
	assert_success
	assert_line "would create $work/.agents/skills/jouet-init/SKILL.md"
	assert_line "would link $work/.claude/skills/jouet-init"
	assert [ ! -e .claude/skills/jouet-init ]
	jouet setup claude --shared --init-skill >/dev/null
	jouet setup codex --shared --init-skill >/dev/null
	# Uninstalling Codex leaves the copy Claude Code still reads.
	run --separate-stderr jouet setup codex --shared --init-skill --uninstall
	assert_success
	assert_line "kept $work/.agents/skills/jouet-init/SKILL.md"
	assert [ -f .claude/skills/jouet-init/SKILL.md ]
	# A dangling link from an older jouet is made whole again.
	rm -r .agents/skills/jouet-init
	jouet setup claude --shared --init-skill >/dev/null
	assert [ -L .claude/skills/jouet-init ]
	assert [ -f .claude/skills/jouet-init/SKILL.md ]
}

@test "uninstalling Codex keeps the skill Claude Code reads through a linked .claude/skills" {
	init_repo
	work=$(pwd -P)
	mkdir -p .agents/skills .claude
	ln -s ../.agents/skills .claude/skills
	jouet setup codex --shared --init-skill >/dev/null
	jouet setup claude --shared --init-skill >/dev/null
	assert [ ! -L .claude/skills/jouet-init ]
	run --separate-stderr jouet setup codex --shared --init-skill --uninstall
	assert_success
	assert_line "kept $work/.agents/skills/jouet-init/SKILL.md"
	assert [ -f .claude/skills/jouet-init/SKILL.md ]
}

@test "a link of your own to another skill is never replaced or removed" {
	init_repo
	work=$(pwd -P)
	mkdir -p mine/jouet-init .claude/skills
	echo "my skill" >mine/jouet-init/SKILL.md
	ln -s ../../mine/jouet-init .claude/skills/jouet-init
	run --separate-stderr jouet setup claude --shared --init-skill
	assert_success
	assert_line "kept $work/.claude/skills/jouet-init/SKILL.md"
	assert_equal "$(readlink .claude/skills/jouet-init)" ../../mine/jouet-init
	run --separate-stderr jouet setup claude --shared --init-skill --uninstall
	assert_success
	assert_equal "$(readlink .claude/skills/jouet-init)" ../../mine/jouet-init
	assert_content mine/jouet-init/SKILL.md $'my skill\n'
}

@test "uninstalling Claude Code keeps the skill another harness reads through a linked .claude/skills" {
	init_repo
	work=$(pwd -P)
	mkdir -p .agents/skills .claude
	ln -s ../.agents/skills .claude/skills
	jouet setup codex --shared --init-skill >/dev/null
	jouet setup claude --shared --init-skill >/dev/null
	run --separate-stderr jouet setup claude --shared --init-skill --uninstall
	assert_success
	assert_line "kept $work/.claude/skills/jouet-init/SKILL.md"
	assert [ -f .agents/skills/jouet-init/SKILL.md ]
}

@test "an earlier setup's own Claude Code skill directory becomes the link" {
	init_repo
	jouet setup claude --shared --init-skill >/dev/null
	rm .claude/skills/jouet-init
	mkdir -p .claude/skills/jouet-init
	cp .agents/skills/jouet-init/SKILL.md .claude/skills/jouet-init/SKILL.md
	rm -r .agents/skills/jouet-init
	jouet setup claude --shared --init-skill >/dev/null
	assert [ -L .claude/skills/jouet-init ]
	assert [ -f .agents/skills/jouet-init/SKILL.md ]
	# A directory of someone else's is left as it is.
	rm .claude/skills/jouet-init
	mkdir -p .claude/skills/jouet-init
	echo "mine" >.claude/skills/jouet-init/notes.md
	jouet setup claude --shared --init-skill >/dev/null
	assert [ ! -L .claude/skills/jouet-init ]
	assert [ -f .claude/skills/jouet-init/notes.md ]
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
	# The shared install's link, and the copy it names, which nothing else reads.
	assert_line "removed $work/.claude/skills/jouet"
	assert_line "removed $work/.agents/skills/jouet/SKILL.md"
	assert [ ! -e .claude/skills/jouet ]
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

	# A jouet that cannot run, like a mise shim for a version not installed.
	printf '#!/bin/sh\necho "mise ERROR jouet@0.0.3 is not installed" >&2\nexit 1\n' >"$BATS_TEST_TMPDIR/sh-only/jouet"
	chmod +x "$BATS_TEST_TMPDIR/sh-only/jouet"
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

@test "setup cursor, gemini and copilot write each harness's own hooks file" {
	run --separate-stderr jouet setup cursor
	assert_success
	assert_line "created $HOME/.cursor/hooks.json"
	assert_golden "$HOME/.cursor/hooks.json" cursor-hooks.json

	run --separate-stderr jouet setup gemini
	assert_success
	assert_line "created $HOME/.gemini/settings.json"
	assert_line "note: Gemini CLI runs hooks from settings files only in a trusted folder"
	assert_golden "$HOME/.gemini/settings.json" gemini-settings.json

	COPILOT_HOME="$BATS_TEST_TMPDIR/copilot" run --separate-stderr jouet setup copilot
	assert_success
	assert_line "created $BATS_TEST_TMPDIR/copilot/hooks/jouet.json"
	assert_golden "$BATS_TEST_TMPDIR/copilot/hooks/jouet.json" copilot-hooks.json
}

@test "setup gemini merges into existing settings, and a re-run and uninstall keep them" {
	mkdir -p "$HOME/.gemini"
	printf '%s\n' '{"ui":{"theme":"GitHub"},"hooks":{"AfterAgent":[{"hooks":[{"type":"command","command":"notify-send done"}]}]}}' \
		>"$HOME/.gemini/settings.json"

	run --separate-stderr jouet setup gemini
	assert_success
	run jq -c '[.ui.theme, [.hooks.AfterAgent[].hooks[].command]]' "$HOME/.gemini/settings.json"
	assert_output '["GitHub",["notify-send done","sh -c '"'"'jouet --version >/dev/null 2>&1 || { echo {}; exit 0; }; exec jouet hook gemini stop'"'"'"]]'

	run --separate-stderr jouet setup gemini
	assert_line "unchanged $HOME/.gemini/settings.json"

	run --separate-stderr jouet setup gemini --uninstall
	assert_success
	run jq -c . "$HOME/.gemini/settings.json"
	assert_output '{"ui":{"theme":"GitHub"},"hooks":{"AfterAgent":[{"hooks":[{"type":"command","command":"notify-send done"}]}]}}'
}

@test "setup cursor keeps other hooks and the version, and uninstalling removes only jouet's" {
	mkdir -p "$HOME/.cursor"
	printf '%s\n' '{"version":1,"hooks":{"beforeShellExecution":[{"command":"./approve.sh","matcher":"curl"}],"stop":[{"command":"./notify.sh"}]}}' \
		>"$HOME/.cursor/hooks.json"

	run --separate-stderr jouet setup cursor
	assert_success
	run jq -c '[.version, [.hooks.stop[].command]]' "$HOME/.cursor/hooks.json"
	assert_output '[1,["./notify.sh","sh -c '"'"'jouet --version >/dev/null 2>&1 || { echo {}; exit 0; }; exec jouet hook cursor stop'"'"'"]]'

	run --separate-stderr jouet setup cursor --uninstall
	run jq -c . "$HOME/.cursor/hooks.json"
	assert_output '{"version":1,"hooks":{"beforeShellExecution":[{"command":"./approve.sh","matcher":"curl"}],"stop":[{"command":"./notify.sh"}]}}'
}

@test "setup copilot owns hooks/jouet.json, which uninstalling removes" {
	run --separate-stderr jouet setup copilot --dry-run
	assert_output --partial "would create $HOME/.copilot/hooks/jouet.json"
	assert [ ! -e "$HOME/.copilot" ]

	jouet setup copilot >/dev/null
	run --separate-stderr jouet setup copilot --uninstall
	assert_success
	assert_line "removed $HOME/.copilot/hooks/jouet.json"
	assert [ ! -e "$HOME/.copilot/hooks/jouet.json" ]
}

@test "setup refuses --project for harnesses with no hooks file of your own, and --shared writes the repository's" {
	init_repo
	work=$(pwd -P) # macOS reports /private/tmp for /tmp

	run --separate-stderr jouet setup cursor --project
	assert_failure 2
	assert_equal "$stderr" "jouet: Cursor has no project hooks file of your own: .cursor/hooks.json is the repository's, shared with everyone who clones it. Use --global, or --project --shared to install there anyway"
	run --separate-stderr jouet setup gemini --project
	assert_failure 2
	run --separate-stderr jouet setup copilot --project
	assert_failure 2
	assert [ ! -e .cursor ] && assert [ ! -e .gemini ] && assert [ ! -e .github ]

	run --separate-stderr jouet setup cursor --shared
	assert_success
	assert_line "created $work/.cursor/hooks.json"
	assert_line "note: Cursor runs a project's hooks only once the workspace is trusted"
	run --separate-stderr jouet setup gemini --shared
	assert_line "created $work/.gemini/settings.json"
	run --separate-stderr jouet setup copilot --shared --skill
	assert_success
	assert_line "created $work/.github/hooks/jouet.json"
	assert_line "created $work/.agents/skills/jouet/SKILL.md"
	assert_line --partial "cloud agent uses .github/hooks/ only from the default branch"
}

@test "setup cursor says that Cursor also runs jouet's Claude Code hooks" {
	jouet setup claude >/dev/null

	run --separate-stderr jouet setup cursor

	assert_success
	assert_line "note: Cursor also runs the Claude Code hooks in $HOME/.claude/settings.json: while these are installed, those step aside in Cursor sessions, so each call gets one verdict"
}

@test "an installed Cursor, Gemini CLI or Copilot hook answers {} once jouet is not on PATH" {
	jouet setup cursor >/dev/null
	jouet setup gemini >/dev/null
	jouet setup copilot >/dev/null
	mkdir -p "$BATS_TEST_TMPDIR/sh-only"
	ln -s "$(command -v sh)" "$BATS_TEST_TMPDIR/sh-only/sh"
	input='{"session_id":"s1","cwd":"/tmp","tool_name":"run_shell_command","tool_input":{"command":"git add -A"}}'

	for command in \
		"$(jq -r '.hooks.preToolUse[0].command' "$HOME/.cursor/hooks.json")" \
		"$(jq -r '.hooks.BeforeTool[0].hooks[0].command' "$HOME/.gemini/settings.json")" \
		"$(jq -r '.hooks.preToolUse[0].bash' "$HOME/.copilot/hooks/jouet.json")"; do
		run --separate-stderr sh -c "$command" <<<"$input"
		assert_success
		refute_output ""

		run --separate-stderr env PATH="$BATS_TEST_TMPDIR/sh-only" sh -c "$command" <<<"$input"
		assert_success
		assert_output '{}'
		assert_equal "$stderr" ""
	done
	# Gemini CLI's own tool names reach jouet's rules.
	command=$(jq -r '.hooks.BeforeTool[0].hooks[0].command' "$HOME/.gemini/settings.json")
	run --separate-stderr sh -c "$command" <<<"$input"
	assert_output --partial '"decision":"deny"'
}

# Writes N Claude Code sessions under $HOME/.claude/projects that carried the
# Bash and Workflow tools, the linear MCP server the person configured, the
# app's visualize server, a plugin's docx skill, the bundled loop skill and
# the person's own update-aube skill, and called only Bash, each reaching
# CONTEXT tokens. Only linear and the docx skill are the person's to deny.
claude_sessions() {
	local n=$1 context=${2:-50000} dir="$HOME/.claude/projects/-src-app" i
	mkdir -p "$dir"
	jq -n '{mcpServers: {linear: {command: "linear-mcp"}}}' >"$HOME/.claude.json"
	for ((i = 0; i < n; i++)); do
		{
			jq -nc '{type: "attachment", isSidechain: false, attachment: {type: "prompt_snapshot", tools: [
				{name: "Bash", description: "Runs a command", input_schema: {}},
				{name: "Workflow", description: ("x" * 4600), input_schema: {}},
				{name: "mcp__linear__list_issues", description: ("z" * 2300), input_schema: {}},
				{name: "mcp__visualize__show_widget", description: ("y" * 2300), input_schema: {}}]}}'
			jq -nc '{type: "attachment", isSidechain: false, attachment: {type: "skill_listing",
				content: "- anthropic-skills:docx: Word documents\n- loop: run a prompt on an interval\n- update-aube: update the embedded aube crates"}}'
			jq -nc --argjson c "$context" '{type: "assistant", message: {usage: {input_tokens: 0, cache_read_input_tokens: $c},
				content: [{type: "tool_use", name: "Bash", input: {command: "ls"}}]}}'
		} >"$dir/session-$i.jsonl"
	done
	jq -nc '{type: "user", message: {content: "<command-name>/loop</command-name>"}}' >>"$dir/session-0.jsonl"
}

@test "setup applies the context budget, says why and how to take it back" {
	claude_sessions 20 640000
	run --separate-stderr jouet setup claude --dry-run --json
	assert_success
	run jq -c '.context_budget | [.compact_window, .past_window, [.unused[] | [.kind, .name]]]' <<<"$output"
	assert_output '[200000,20,[["mcp","linear"],["skill","anthropic-skills:docx"]]]'
	assert [ ! -e "$HOME/.claude/settings.json" ]

	run --separate-stderr jouet setup claude
	assert_success
	assert_output --partial "note: setup cut what each request carries, from your recent sessions: compact at 200k tokens (20 of your last 20 sessions went past 200k tokens of context, up to 640k); and deny 2 definitions none of your last 20 sessions used"
	assert_output --partial 'MCP servers linear; 1 skills;'
	assert_output --partial "\`jouet setup claude --no-context-budget\` installs without it"
	assert_output --partial "note: set autoCompactWindow to 200000; \`jouet setup claude --undo window\` takes it back"
	assert_output --partial "\`jouet setup claude --undo unused\` takes them back"
	run jq -c '[.autoCompactWindow, .permissions.deny, .skillOverrides]' "$HOME/.claude/settings.json"
	assert_output '[200000,["Artifact","ArtifactComments","ArtifactData","mcp__linear"],{"anthropic-skills:docx":"user-invocable-only"}]'

	# Running it again changes nothing.
	run --separate-stderr jouet setup claude
	assert_success
	refute_output --partial 'setup cut'
}

@test "an undone part of the context budget stays undone, and --no-context-budget skips it" {
	claude_sessions 20 640000
	run --separate-stderr jouet setup claude --no-context-budget
	assert_success
	refute_output --partial 'setup cut'
	run jq -c '[.autoCompactWindow, .permissions.deny, .skillOverrides]' "$HOME/.claude/settings.json"
	assert_output '[null,["Artifact","ArtifactComments","ArtifactData"],null]'

	run --separate-stderr jouet setup claude
	run --separate-stderr jouet setup claude --undo window
	assert_success
	run --separate-stderr jouet setup claude
	assert_success
	refute_output --partial 'compact at'
	run jq -c '[.autoCompactWindow, (.permissions.deny | length)]' "$HOME/.claude/settings.json"
	assert_output '[null,4]'

	run --separate-stderr jouet setup claude --undo unused
	run --separate-stderr jouet setup claude
	refute_output --partial 'setup cut'
	run jq -c '[.autoCompactWindow, .permissions.deny, .skillOverrides]' "$HOME/.claude/settings.json"
	assert_output '[null,["Artifact","ArtifactComments","ArtifactData"],null]'

	# Its flag asks for it again.
	run --separate-stderr jouet setup claude --compact-window 200k
	run --separate-stderr jouet setup claude
	run jq .autoCompactWindow "$HOME/.claude/settings.json"
	assert_output 200000
}

@test "a 200k model's sessions are not offered a window, and few sessions no denials" {
	claude_sessions 19 150000
	run --separate-stderr jouet setup claude
	assert_success
	refute_output --partial 'setup cut'
	run --separate-stderr jouet setup claude --deny-unused
	assert_success
	assert_output --partial 'note: nothing to deny: only 19 recent sessions recorded their tools, and it takes 20'
}

@test "each context budget choice is one undo, and uninstall takes back both" {
	claude_sessions 20
	jq -n '{permissions: {deny: ["Bash(rm:*)"]}, skillOverrides: {loop: "off"}}' >"$HOME/.claude/settings.json"
	run --separate-stderr jouet setup claude --compact-window 300k --deny-unused
	assert_success
	assert_output --partial "note: set autoCompactWindow to 300000; \`jouet setup claude --undo window\` takes it back"
	assert_output --partial 'note: denied 1 unused MCP servers and hid 1 unused skills from the model'
	run jq -c '[.autoCompactWindow, .permissions.deny, .skillOverrides]' "$HOME/.claude/settings.json"
	assert_output '[300000,["Bash(rm:*)","Artifact","ArtifactComments","ArtifactData","mcp__linear"],{"loop":"off","anthropic-skills:docx":"user-invocable-only"}]'

	run --separate-stderr jouet setup claude --undo window --dry-run
	assert_success
	assert_line "note: would remove the autoCompactWindow of 300000 that setup set"
	run jq .autoCompactWindow "$HOME/.claude/settings.json"
	assert_output 300000

	run --separate-stderr jouet setup claude --undo window
	assert_success
	assert_line "note: removed the autoCompactWindow of 300000 that setup set"
	run jq -c '[.autoCompactWindow, (.permissions.deny | length)]' "$HOME/.claude/settings.json"
	assert_output '[null,5]'

	run --separate-stderr jouet setup claude --undo unused
	assert_success
	run jq -c '[.permissions.deny, .skillOverrides]' "$HOME/.claude/settings.json"
	assert_output '[["Bash(rm:*)","Artifact","ArtifactComments","ArtifactData"],{"loop":"off"}]'

	run --separate-stderr jouet setup claude --compact-window 200k --deny-unused
	run --separate-stderr jouet setup claude --uninstall
	assert_success
	run jq -c . "$HOME/.claude/settings.json"
	assert_output '{"permissions":{"deny":["Bash(rm:*)"]},"skillOverrides":{"loop":"off"}}'
}

@test "setup puts back what an earlier run took for unused but the app or the person depends on" {
	claude_sessions 20
	jq -n '{permissions: {deny: ["ReportFindings", "mcp__visualize", "mcp__linear"]},
		skillOverrides: {"update-aube": "user-invocable-only", "loop": "user-invocable-only"}}' >"$HOME/.claude/settings.json"
	mkdir -p "$XDG_STATE_HOME/jouet"
	jq -n --arg s "$HOME/.claude/settings.json" '{($s): {deny: ["ReportFindings", "mcp__visualize", "mcp__linear"],
		skills: ["update-aube", "loop"]}}' >"$XDG_STATE_HOME/jouet/setup-budget.json"

	run --separate-stderr jouet setup claude --deny-unused
	assert_success
	assert_output --partial "note: put back ReportFindings, mcp__visualize, update-aube, loop: setup took them for unused, but they are Claude Code's, the app's or your own"
	assert_output --partial 'note: denied 0 unused MCP servers and hid 1 unused skills from the model'
	run jq -c '[.permissions.deny, .skillOverrides]' "$HOME/.claude/settings.json"
	assert_output '[["mcp__linear","Artifact","ArtifactComments","ArtifactData"],{"anthropic-skills:docx":"user-invocable-only"}]'

	# What it keeps is still setup's to undo.
	run --separate-stderr jouet setup claude --undo unused
	run jq -c '[.permissions.deny, .skillOverrides]' "$HOME/.claude/settings.json"
	assert_output '[["Artifact","ArtifactComments","ArtifactData"],null]'
}

@test "the context budget flags are for Claude Code's own settings" {
	run --separate-stderr jouet setup codex --deny-unused
	assert_failure 2
	run --separate-stderr jouet setup claude --shared --compact-window 200k
	assert_failure 2
	run --separate-stderr jouet setup claude --compact-window 50k
	assert_failure 2
	assert_stderr --partial 'from 100k to 1m tokens'
	run --separate-stderr jouet setup claude --uninstall --undo window
	assert_failure 2
}

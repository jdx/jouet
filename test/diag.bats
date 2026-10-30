#!/usr/bin/env bats

# `jouet diag` on a fixture repository with an hk.pkl, run through the hk
# installed with mise: a builtin step whose tool is a fake (shellcheck), a
# step that reports gcc-style diagnostics, and one that sets no
# `diagnostic_format`.

setup() {
	# Found before the test's HOME replaces the developer's, which mise's
	# shims would need.
	HK_BIN="$(mise which hk 2>/dev/null || command -v hk || true)"
	load "test_helper/common_setup"
	_common_setup
	unset JOUET_HOOKS JOUET_HOOKS_VERIFY JOUET_HOOKS_LEDGER JOUET_EDIT_FIX_AFTER JOUET_EDIT_FIX_STEPS HK_FILE
	export XDG_CONFIG_HOME="$BATS_TEST_TMPDIR/config"
	if [[ -z "$HK_BIN" ]]; then
		skip "hk is not installed"
	fi
	export JOUET_HK="$HK_BIN"
	HK_VERSION="$("$HK_BIN" --version | awk '{print $2}')"
	export HK_LOG_DIR="$BATS_TEST_TMPDIR/hk-logs" HK_STATE_DIR="$BATS_TEST_TMPDIR/hk-state" HK_CACHE_DIR="$BATS_TEST_TMPDIR/hk-cache"
	fake_tools
	fixture
}

# A shellcheck that reports SC2086 on every `echo $` line, in its default
# output or its JSON one.
fake_tools() {
	mkdir -p "$BATS_TEST_TMPDIR/tools"
	cat >"$BATS_TEST_TMPDIR/tools/shellcheck" <<'SH'
#!/usr/bin/env bash
json=0
files=()
for a in "$@"; do
	case "$a" in
	--format=json1) json=1 ;;
	-*) ;;
	*) files+=("$a") ;;
	esac
done
status=0
comments=()
for f in "${files[@]}"; do
	n=0
	while IFS= read -r line; do
		n=$((n + 1))
		[[ $line == 'echo $'* ]] || continue
		status=1
		if ((json)); then
			comments+=("{\"file\":\"$f\",\"line\":$n,\"column\":6,\"level\":\"info\",\"code\":2086,\"message\":\"Double quote to prevent globbing and word splitting.\"}")
		else
			printf '\nIn %s line %d:\n%s\n     ^-- SC2086 (info): Double quote to prevent globbing and word splitting.\n' "$f" "$n" "$line"
		fi
	done <"$f"
done
if ((json)); then
	(IFS=,; printf '{"comments":[%s]}\n' "${comments[*]}")
fi
exit "$status"
SH
	chmod +x "$BATS_TEST_TMPDIR/tools/shellcheck"
	export PATH="$BATS_TEST_TMPDIR/tools:$PATH"
}

# A repository whose hk.pkl has a builtin (shellcheck), a step that prints
# `path:line:col: warning: message [rule]` (todo) and one with no
# diagnostic_format (trailing), with a fix step that strips trailing spaces.
fixture() {
	git init -q repo
	cd repo || return 1
	git config user.email t@example.com
	git config user.name t
	cat >todo.sh <<'SH'
#!/bin/sh
status=0
for f in "$@"; do
	if grep -n TODO "$f" | sed "s|^\([0-9]*\):.*|$f:\1:1: warning: TODO left in [todo]|" | grep .; then
		status=1
	fi
done
exit "$status"
SH
	cat >hk.pkl <<PKL
amends "package://github.com/jdx/hk/releases/download/v$HK_VERSION/hk@$HK_VERSION#/Config.pkl"
import "package://github.com/jdx/hk/releases/download/v$HK_VERSION/hk@$HK_VERSION#/Builtins.pkl"

local linters = new Mapping<String, Step> {
  ["shellcheck"] = Builtins.shellcheck
  ["todo"] = new Step {
    glob = "*.txt"
    check = new CommandSpec { command = "sh todo.sh {{files}}"; effect = "read" }
    diagnostic_format = "gcc"
  }
  ["trailing"] = new Step {
    glob = "*.md"
    check = new CommandSpec { command = "! grep -n ' \$' {{files}}"; effect = "read" }
    fix = new CommandSpec { command = "sed -i.bak 's/ *\$//' {{files}} && rm -f {{files}}.bak"; effect = "write" }
  }
}
hooks {
  ["check"] { steps = linters }
  ["fix"] { fix = true; steps = linters }
}
PKL
	printf 'one\ntwo\n' >notes.txt
	printf '# Notes\n' >README.md
	printf '#!/bin/sh\necho "%s"\n' "\$1" >run.sh
	git add -A
	git commit -qm init
}

@test "check reports each step's findings, and a failed step without diagnostics by its output" {
	printf 'one\nTODO two\n' >notes.txt
	printf '# Notes \n' >README.md

	run --separate-stderr jouet diag check --changed --json

	assert_failure 1
	run jq -c '.status, .files, [.findings[] | select(.source_step == "todo") | {file, line, rule, severity}]' <<<"$output"
	assert_line --index 0 '"findings"'
	assert_line --index 1 '["README.md","notes.txt"]'
	assert_line --index 2 '[{"file":"notes.txt","line":2,"rule":"todo","severity":"warning"}]'
	run jq -r '.steps[] | select(.name == "trailing") | .status, .excerpt' <<<"$(jouet diag check --changed --json)"
	assert_line --index 0 failed
	assert_line --index 1 '1:# Notes'
}

@test "a builtin's findings come back, whether this hk parses its output or not" {
	printf '#!/bin/sh\necho %s\n' "\$1" >run.sh

	run --separate-stderr jouet diag check run.sh

	assert_failure 1
	# hk with a parser for shellcheck's output reports the finding; an older
	# one reports the failed step and its output.
	assert_output --regexp 'run\.sh:2:6: note: .* \[SC2086\] \(shellcheck\)|step shellcheck failed:'
}

@test "a clean file exits 0" {
	printf 'three\n' >>notes.txt

	run --separate-stderr jouet diag check notes.txt

	assert_success
	assert_output --regexp '^clean: 1 file \(hk, [0-9]+ ms\)$'
}

@test "hk refusing a command whose effect is unknown exits 4 with its reason" {
	# A check given as a plain string has no declared effect.
	sed -i.bak 's|check = new CommandSpec { command = "sh todo.sh {{files}}"; effect = "read" }|check = "sh todo.sh {{files}}"|' hk.pkl
	grep -q 'check = "sh todo.sh' hk.pkl
	printf 'three\n' >>notes.txt

	run --separate-stderr jouet diag check notes.txt --json

	assert_failure 4
	run jq -r '.status, .reason' <<<"$output"
	assert_line --index 0 refused
	assert_line --index 1 '--safe refused to run:'
	assert_output --partial 'todo.check: effect is unknown'
}

@test "delta reports the finding an edit introduced, not one that moved" {
	printf 'one\nTODO old\n' >notes.txt
	git commit -qam todo
	printf 'one\nTODO old\nmore\n' >notes.txt
	jouet diag check notes.txt >/dev/null || true
	# Three lines above the old finding, and a new one.
	printf 'a\nb\nc\none\nTODO old\nmore\nTODO new\n' >notes.txt

	run --separate-stderr jouet diag delta notes.txt --json

	assert_failure 1
	run jq -c '.status, [.new[] | {file, line}], .kept, .fixed' <<<"$output"
	assert_line --index 0 '"introduced"'
	assert_line --index 1 '[{"file":"notes.txt","line":7}]'
	assert_line --index 2 1
	assert_line --index 3 0
}

@test "delta counts nothing as new in a file unchanged since HEAD" {
	printf 'one\nTODO old\n' >notes.txt
	git commit -qam todo

	run --separate-stderr jouet diag delta notes.txt

	assert_success
	assert_output --regexp '^0 new, 1 kept, 0 fixed in 1 file'
}

@test "edit --diag fails and reverts only on a finding the edit introduced" {
	printf 'one\nTODO old\n' >notes.txt
	git commit -qam todo

	run --separate-stderr jouet edit replace notes.txt --old one --new $'zero\none' --diag --revert-on-fail
	assert_success
	assert_line 'diag: 0 new, 1 kept, 0 fixed'

	run --separate-stderr jouet edit replace notes.txt --old zero --new 'TODO zero' --diag --revert-on-fail
	assert_failure 1
	assert_line 'new: notes.txt:1:1: warning: TODO left in [todo] (todo)'
	assert_line --partial 'reverted'
	assert_content notes.txt $'zero\none\nTODO old\n'
}

@test "fix_after formats what jouet edit writes, and the post-edit hook what the harness wrote" {
	export JOUET_EDIT_FIX_AFTER=1

	run --separate-stderr jouet edit replace README.md --old Notes --new 'Notes  '
	assert_success
	assert_line 'hk fix formatted 1 file (README.md); reread before editing them again'
	assert_content README.md $'# Notes\n'

	printf '# Notes   \n' >README.md
	run --separate-stderr jouet hook claude post-edit <<<"$(jq -cn --arg cwd "$PWD" \
		'{session_id: "s1", cwd: $cwd, tool_name: "Edit", tool_input: {file_path: "README.md"}}')"
	assert_success
	run jq -r '.hookSpecificOutput.additionalContext' <<<"$output"
	assert_output 'jouet: hk fix formatted 1 file (README.md); reread before editing them again'
	assert_content README.md $'# Notes\n'
	# The ledger keeps how many files it changed, never their names.
	run jq -c 'select(.event == "fix") | [.tool, .formatted, .decisions[0].scope]' "$XDG_STATE_HOME/jouet/ledger/ledger.jsonl"
	assert_line '["Edit",1,"ran"]'
	refute_output --partial README

	# Off, the hook does nothing.
	unset JOUET_EDIT_FIX_AFTER
	printf '# Notes   \n' >README.md
	run --separate-stderr jouet hook claude post-edit <<<"$(jq -cn --arg cwd "$PWD" \
		'{session_id: "s1", cwd: $cwd, tool_name: "Edit", tool_input: {file_path: "README.md"}}')"
	assert_output ""
	assert_content README.md $'# Notes   \n'
}

@test "diag fix runs only hk's fix steps on the files given" {
	printf '# Notes  \n' >README.md
	printf 'x  \n' >other.md

	run --separate-stderr jouet diag fix README.md --json

	assert_success
	run jq -c '.status, .formatted' <<<"$output"
	assert_line --index 0 '"ran"'
	assert_line --index 1 '["README.md"]'
	assert_content other.md $'x  \n'
}

# A stop hook standing in for `hk agent stop-hook`: blocks with a reason.
fake_hk_stop() {
	printf '#!/bin/sh\ncat >/dev/null\necho '"'"'{"decision":"block","reason":"hk check failed: todo"}'"'"'\n' >"$BATS_TEST_TMPDIR/tools/hk-stop"
	chmod +x "$BATS_TEST_TMPDIR/tools/hk-stop"
}

@test "the stop hook gives one verdict for jouet's verify and hk's stop hook" {
	fake_hk_stop
	payload() {
		jq -cn --arg cwd "$PWD" '{session_id: "s1", cwd: $cwd, stop_hook_active: false}'
	}

	run --separate-stderr jouet hook claude stop --hk hk-stop <<<"$(payload)"
	assert_success
	run jq -r '.decision, .reason' <<<"$output"
	assert_line --index 0 block
	assert_line --index 1 'hk check failed: todo [hk]'

	# A failed test run too: both reasons, in one block.
	jq -cn --arg cwd "$PWD" '{session_id: "s1", cwd: $cwd, hook_event_name: "PostToolUseFailure", tool_name: "Bash",
		tool_input: {command: "cargo test --workspace"}, error: "Exit code 101"}' |
		jouet hook claude post-tool-use-failure
	run --separate-stderr jouet hook claude stop --hk hk-stop <<<"$(payload)"
	run jq -r '.reason' <<<"$output"
	assert_line --index 0 --partial "\`cargo test --workspace\` exited 101"
	assert_line --partial 'hk check: hk check failed: todo [hk]'
}

@test "a passing hk check at the current tree spares the stop hook from running hk again" {
	fake_hk_stop
	printf 'three\n' >>notes.txt
	jq -cn --arg cwd "$PWD" '{session_id: "s1", cwd: $cwd, hook_event_name: "PostToolUse", tool_name: "Bash",
		tool_input: {command: "jouet diag check --changed"}, tool_response: {stdout: ""}}' |
		jouet hook claude post-tool-use

	run --separate-stderr jouet hook claude stop --hk hk-stop <<<"$(jq -cn --arg cwd "$PWD" '{session_id: "s1", cwd: $cwd, stop_hook_active: false}')"
	assert_success
	assert_output ""

	# A file changed since: hk runs, and jouet's verify asks for a new check.
	printf 'four\n' >>notes.txt
	run --separate-stderr jouet hook claude stop --hk hk-stop <<<"$(jq -cn --arg cwd "$PWD" '{session_id: "s1", cwd: $cwd, stop_hook_active: false}')"
	run jq -r '.reason' <<<"$output"
	assert_line --index 0 --partial "Files changed after the last hk check passed (\`jouet diag check --changed\`)"
	assert_line --partial 'hk check: hk check failed: todo [hk]'
}

@test "setup merges hk's stop hook into jouet's, and uninstall gives it back" {
	mkdir -p .claude
	hk_hook='{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"hk agent stop-hook --target claude-code --timeout 570","timeout":600}]}]}}'
	printf '%s\n' "$hk_hook" >.claude/settings.json

	run --separate-stderr jouet setup claude --project --shared --json
	assert_success
	assert_output --partial "hk's stop hook in"
	run jq -c '[.hooks.Stop[].hooks[] | {command, timeout}]' .claude/settings.json
	assert_output '[{"command":"sh -c '"'"'command -v jouet >/dev/null 2>&1 || exec hk agent stop-hook --target claude-code --timeout 570; exec jouet hook claude stop --hk \"hk agent stop-hook --target claude-code --timeout 570\"'"'"'","timeout":600}]'

	run --separate-stderr jouet setup claude --project --shared --uninstall
	assert_success
	run jq -c . .claude/settings.json
	assert_output "$hk_hook"
}

@test "without an hk config, a shell script falls back to shellcheck and other files are unavailable" {
	git rm -q hk.pkl
	printf '#!/bin/sh\necho %s\n' "\$1" >run.sh

	run --separate-stderr jouet diag check run.sh --json
	assert_failure 1
	run jq -c '.source, [.findings[] | {file, line, rule}]' <<<"$output"
	assert_line --index 0 '"shellcheck"'
	assert_line --index 1 '[{"file":"run.sh","line":2,"rule":"SC2086"}]'

	run --separate-stderr jouet diag check notes.txt
	assert_failure 4
	assert_output --partial 'diagnostics unavailable: no hk config (hk.pkl) here, and no fallback checker for notes.txt'
}

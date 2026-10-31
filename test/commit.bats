#!/usr/bin/env bats

# `jouet commit` as an agent runs it in a checkout other sessions share: only
# the named paths are committed, whatever else is staged or lying around.

setup() {
	load "test_helper/common_setup"
	_common_setup
	init_repo
	commit_file a.txt $'a\n' "init"
}

@test "only the named paths are committed, with the agent's trailers" {
	printf 'a2\n' >a.txt
	mkdir -p src && printf 'new\n' >src/new.rs
	printf 'b\n' >b.txt && git add b.txt
	mkdir -p test/.cache && printf 'junk\n' >test/.cache/blob

	AI_AGENT=claude-code_2-1-309_agent run --separate-stderr jouet commit a.txt src -m "feat: add new"

	assert_success
	assert_line --index 0 --regexp '^committed [0-9a-f]{12} on main: feat: add new$'
	assert_line --index 1 "  modified  a.txt       +1 -1"
	assert_line --index 2 "  added     src/new.rs  +1 -0"
	assert_equal "$(git log -1 --format=%B)" "feat: add new

Co-Authored-By: Claude <noreply@anthropic.com>
AI-Assisted: Tool: Claude Code; model: unavailable; version: 2.1.309"
	assert_equal "$(git status --porcelain --untracked-files=all)" "A  b.txt
?? test/.cache/blob"
}

@test "-A, a bare dot and --no-verify are refused before anything changes" {
	printf 'a2\n' >a.txt

	run --separate-stderr jouet commit -A -m "everything"
	assert_failure 2
	assert_equal "$stderr" "jouet: jouet commit does not take -A or --all
hint: name the paths to commit, so nothing else in the checkout is swept up"

	run --separate-stderr jouet commit . -m "everything"
	assert_failure 2

	run --separate-stderr jouet commit a.txt --no-verify -m "skip hooks"
	assert_failure 2

	assert_equal "$(git rev-list --count HEAD)" 1
}

@test "a path with no changes is refused with exit 4" {
	commit_file b.txt $'b\n' "add b"
	printf 'a2\n' >a.txt

	run --separate-stderr jouet commit a.txt b.txt -m "both"

	assert_failure 4
	assert_equal "$stderr" "jouet: \`b.txt\` has no changes to commit
hint: name only the paths you changed"
}

@test "a failing hook's output is shown and its full log kept" {
	mkdir -p .git/hooks
	cat >.git/hooks/pre-commit <<'HOOK'
#!/bin/sh
for i in $(seq 1 45); do echo "check $i"; done
echo "a.txt: bad" >&2
exit 1
HOOK
	chmod +x .git/hooks/pre-commit
	printf 'a2\n' >a.txt

	run --separate-stderr jouet commit a.txt -m "change"

	assert_failure 1
	assert_line --index 0 "commit failed (exit 1); nothing was committed and nothing was left staged"
	assert_line --index 1 "  ... 6 earlier lines"
	assert_line --index 41 "  a.txt: bad"
	local log
	log="$(sed -n 's/^  full log: //p' <<<"$output")"
	[[ "$log" == "$JOUET_LOG_DIR"/commit-*.log ]]
	assert_equal "$(head -1 "$log")" "check 1"
	assert_equal "$(git rev-list --count HEAD)" 1
}

@test "a message from a heredoc, with --json and no trailers" {
	printf 'a2\n' >a.txt

	# Set but empty: no trailers, whichever agent runs the tests.
	JOUET_COMMIT_TRAILERS='' run --separate-stderr jouet commit a.txt -F - --json <<'MSG'
fix: keep the message exactly

Body with `backticks` and "quotes".
MSG

	assert_success
	assert_equal "$(jq -r '.files[0].path' <<<"$output")" a.txt
	assert_equal "$(jq -r .subject <<<"$output")" "fix: keep the message exactly"
	assert_equal "$(git log -1 --format=%b)" "Body with \`backticks\` and \"quotes\"."
}

@test "in a fnox project, fnox scans exactly what is committed" {
	fake_fnox
	printf '[secrets]\nSTRIPE_KEY = { provider = "age", value = "YWdl" }\n' >fnox.toml
	git add fnox.toml && git commit -q -m "add fnox"
	printf 'a2\n' >a.txt
	# Staged by someone else, with the value: not part of this commit.
	printf 'key sk_test_value_123\n' >other.txt && git add other.txt
	printf 'FAKE_FNOX_VALUE=sk_test_value_123\nFAKE_FNOX_NAME=STRIPE_KEY\n' >"$BATS_TEST_TMPDIR/fnox.env"

	JOUET_COMMIT_TRAILERS='' run --separate-stderr jouet commit a.txt -m "fix: a"

	assert_success
	assert_line --index 1 "  modified  a.txt  +1 -1"
	assert_line --index 2 "  fnox found no secret values in 1 file"
	assert_equal "$(cat "$BATS_TEST_TMPDIR/fnox.log")" "scan --values --staged --format json"
	assert_equal "$(git status --porcelain)" "A  other.txt"
}

@test "a secret's value refuses the commit with exit 1, naming the secret and file but never the value" {
	fake_fnox
	printf '[secrets]\nSTRIPE_KEY = { provider = "age", value = "YWdl" }\n' >fnox.toml
	git add fnox.toml && git commit -q -m "add fnox"
	printf 'a2\n' >a.txt
	mkdir -p config && printf 'STRIPE=sk_test_value_123\n' >config/app.env
	printf 'FAKE_FNOX_VALUE=sk_test_value_123\nFAKE_FNOX_NAME=STRIPE_KEY\n' >"$BATS_TEST_TMPDIR/fnox.env"

	run --separate-stderr jouet commit a.txt config -m "feat: config"

	assert_failure 1
	assert_line --index 0 "commit refused: fnox found the value of a secret in what would be committed; nothing was committed and nothing was left staged"
	assert_line --index 1 "  STRIPE_KEY  config/app.env:1"
	refute_output --partial sk_test_value_123
	assert_equal "$(git rev-list --count HEAD)" 2
	assert_equal "$(git status --porcelain --untracked-files=all)" " M a.txt
?? config/app.env"

	run --separate-stderr jouet --json commit config -m "feat: config"
	assert_failure 1
	run jq -c '[.committed, .secret_scan.status, .secret_scan.findings]' <<<"$output"
	assert_output '[null,"found",[{"secret":"STRIPE_KEY","path":"config/app.env","line":1}]]'
}

@test "a scan fnox cannot run refuses with exit 4, unless the agent runs under fnox proxy run" {
	fake_fnox
	printf '[secrets]\nSTRIPE_KEY = { provider = "age", value = "YWdl" }\n' >fnox.toml
	printf 'a2\n' >a.txt
	printf '%s\n' "FAKE_FNOX_ERROR=\"None of the profile's secrets could be resolved\"" >"$BATS_TEST_TMPDIR/fnox.env"

	run --separate-stderr jouet commit a.txt -m "fix: a"

	assert_failure 4
	assert_equal "${stderr_lines[0]}" "jouet: fnox could not scan the commit for secret values, so nothing was committed: Configuration error: None of the profile's secrets could be resolved (exit 1)"
	assert_equal "$(git rev-list --count HEAD)" 1

	JOUET_COMMIT_TRAILERS='' FNOX_PROXY_MCP=1 run --separate-stderr jouet commit a.txt -m "fix: a"
	assert_success
	assert_equal "$stderr" "jouet: warning: fnox scan failed (Configuration error: None of the profile's secrets could be resolved (exit 1)); the agent runs under fnox proxy run, so it was only given placeholders"
}

@test "without fnox installed the commit goes ahead and says it was not scanned; config can turn the scan off" {
	printf '[secrets]\nSTRIPE_KEY = { provider = "age", value = "YWdl" }\n' >fnox.toml
	printf 'a2\n' >a.txt

	JOUET_COMMIT_TRAILERS='' PATH="$BATS_TEST_TMPDIR/bin:/usr/bin:/bin" run --separate-stderr jouet --json commit a.txt -m "fix: a"

	assert_success
	run jq -r '.secret_scan.status, .warnings[0]' <<<"$output"
	assert_line --index 0 skipped
	assert_line --index 1 "this repository uses fnox, but fnox is not installed, so the commit was not scanned for secret values"

	fake_fnox
	printf 'a3\n' >a.txt
	JOUET_COMMIT_TRAILERS='' JOUET_COMMIT_SECRET_SCAN=off run --separate-stderr jouet commit a.txt -m "fix: a3"
	assert_success
	assert [ ! -e "$BATS_TEST_TMPDIR/fnox.log" ]
}

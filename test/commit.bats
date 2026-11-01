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
	assert_line --index 2 "  no secrets found by gitleaks' rules or fnox in 1 file"
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

@test "a scan fnox cannot run lets the commit go ahead and says why" {
	fake_fnox
	printf '[secrets]\nSTRIPE_KEY = { provider = "age", value = "YWdl" }\n' >fnox.toml
	printf 'a2\n' >a.txt
	printf '%s\n' "FAKE_FNOX_ERROR=\"None of the profile's secrets could be resolved\"" >"$BATS_TEST_TMPDIR/fnox.env"

	JOUET_COMMIT_TRAILERS='' run --separate-stderr jouet commit a.txt -m "fix: a"

	assert_success
	assert_equal "$stderr" "jouet: warning: fnox could not scan the commit for secret values (Configuration error: None of the profile's secrets could be resolved (exit 1)), so only gitleaks' rules checked it"
	assert_equal "$(git rev-list --count HEAD)" 2

	printf 'a3\n' >a.txt
	JOUET_COMMIT_TRAILERS='' FNOX_PROXY_MCP=1 run --separate-stderr jouet commit a.txt -m "fix: a"
	assert_success
	assert_equal "$stderr" "jouet: warning: fnox scan failed (Configuration error: None of the profile's secrets could be resolved (exit 1)); the agent runs under fnox proxy run, so it was only given placeholders"
}

@test "without fnox installed the commit goes ahead and says fnox did not check it; config can turn the scan off" {
	printf '[secrets]\nSTRIPE_KEY = { provider = "age", value = "YWdl" }\n' >fnox.toml
	printf 'a2\n' >a.txt

	JOUET_COMMIT_TRAILERS='' PATH="$BATS_TEST_TMPDIR/bin:/usr/bin:/bin" run --separate-stderr jouet --json commit a.txt -m "fix: a"

	assert_success
	run jq -r '.secret_scan.tool, .secret_scan.status, .warnings[0]' <<<"$output"
	assert_line --index 0 jouet
	assert_line --index 1 clean
	assert_line --index 2 "this repository uses fnox, but fnox is not installed, so the commit was not checked for its secrets' values (gitleaks' rules still scanned it)"

	fake_fnox
	printf 'a3\n' >a.txt
	JOUET_COMMIT_TRAILERS='' JOUET_COMMIT_SECRET_SCAN=off run --separate-stderr jouet commit a.txt -m "fix: a3"
	assert_success
	assert [ ! -e "$BATS_TEST_TMPDIR/fnox.log" ]
}

# A GitHub classic token's shape with a made-up body, put together here so
# this file holds nothing a scanner takes for a secret.
fake_token() {
	printf 'ghp_%s' "Zq3XvB7mN2kL9pR4tY6wC1dF8gH0jK5sA2eU"
}

@test "gitleaks' rules refuse a token in what is committed, never printing it, and --allow-secret lets one through" {
	printf 'a2\n' >a.txt
	mkdir -p src && printf 'fn x() {}\nconst T: &str = "%s";\n' "$(fake_token)" >src/gh.rs
	# Staged by someone else: not part of this commit.
	printf 'token %s\n' "$(fake_token)" >other.txt && git add other.txt

	JOUET_COMMIT_TRAILERS='' run --separate-stderr jouet commit a.txt src -m "feat: gh"

	assert_failure 1
	assert_line --index 0 "commit refused: gitleaks' rules found a secret in what would be committed; nothing was committed and nothing was left staged"
	assert_line --index 1 "  github-pat  src/gh.rs:2  ghp_****"
	assert_line --index 3 "  or, to pass over it for good, add to .gitleaksignore: src/gh.rs:github-pat:2"
	refute_output --partial Zq3XvB7m
	assert_equal "$(git rev-list --count HEAD)" 1
	assert_equal "$(git status --porcelain --untracked-files=all)" " M a.txt
A  other.txt
?? src/gh.rs"

	run --separate-stderr jouet --json commit src -m "feat: gh"
	assert_failure 1
	run jq -c '.secret_scan | [.tool, .status, .findings]' <<<"$output"
	assert_output '["jouet","found",[{"rule":"github-pat","path":"src/gh.rs","line":2,"redacted":"ghp_****"}]]'

	run --separate-stderr jouet commit src -m "feat: gh" --allow-secret src/gh.rs:2
	assert_failure 2
	assert_equal "$stderr" "jouet: --allow-secret takes RULE:PATH:LINE, as the refusal lists the finding, not \`src/gh.rs:2\`"

	JOUET_COMMIT_TRAILERS='' run --separate-stderr jouet commit a.txt src -m "feat: gh" --allow-secret github-pat:src/gh.rs:2
	assert_success
	assert_line --index 3 "  no secrets found by gitleaks' rules in 2 files"
	assert_line --index 4 "  1 finding passed over (.gitleaksignore or --allow-secret)"
	assert_equal "$(git status --porcelain --untracked-files=all)" "A  other.txt"
}

@test "a token already committed does not refuse later commits of the same file" {
	printf 'const T: &str = "%s"; // gitleaks:allow\n' "$(fake_token)" >a.txt
	JOUET_COMMIT_TRAILERS='' run --separate-stderr jouet commit a.txt -m "chore: known test token"
	assert_success

	# The line is in HEAD now; only the added line is scanned.
	sed -i.bak 's| // gitleaks:allow||' a.txt && rm a.txt.bak
	printf 'more\n' >>a.txt
	JOUET_COMMIT_TRAILERS='' run --separate-stderr jouet commit a.txt -m "fix: a"
	assert_failure 1
	assert_line --index 1 "  github-pat  a.txt:1  ghp_****"
	git checkout -q a.txt
	printf 'more\n' >>a.txt
	JOUET_COMMIT_TRAILERS='' run --separate-stderr jouet commit a.txt -m "fix: a"
	assert_success
}

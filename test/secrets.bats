#!/usr/bin/env bats

# `jouet secrets scan` and the `committed-secrets` hook rule, on fixture
# repositories holding fake secrets in real formats. The fakes are put
# together at run time, so this file holds nothing a scanner takes for a
# secret, and no real secret is ever committed.

setup() {
	load "test_helper/common_setup"
	_common_setup
	unset JOUET_HOOKS JOUET_HOOKS_STEER JOUET_RULES_DISABLE
	export XDG_CONFIG_HOME="$BATS_TEST_TMPDIR/config"
	init_repo
	commit_file README.md $'# demo\n' "init"
}

# A GitHub classic token's shape, with a made-up body.
fake_github() {
	printf 'ghp_%s' "Zq3XvB7mN2kL9pR4tY6wC1dF8gH0jK5sA2eU"
}

# An AWS access key ID's shape that is not AWS's documented example.
fake_aws() {
	printf 'AKIA%s' "Q7ZR4XKD2MWB6TLN"
}

# What only gitleaks' generic rule matches: a high-entropy value named as a key.
fake_generic_line() {
	printf 'api_key = "%s%s"\n' q8Wz3Kx7 Mv2Bn9Rt4Pl6
}

hook() {
	jq -cn --arg c "$1" --arg d "$PWD" '{session_id: "s1", tool_name: "Bash", cwd: $d, tool_input: {command: $c}}' |
		jouet hook claude pre-tool-use
}

@test "files: each finding's rule, file and line, redacted, and exit 1; the secret is never printed" {
	mkdir -p config src
	printf 'port = 8080\ntoken = "%s"\n' "$(fake_github)" >config/app.toml
	printf 'const KEY: &str = "%s";\n' "$(fake_aws)" >src/aws.rs

	run --separate-stderr jouet secrets scan

	assert_failure 1
	assert_line --index 0 "found 2 secrets in 3 files (gitleaks v8.30.1, 222 rules)"
	assert_line --index 1 "  config/app.toml:2  github-pat        ghp_****"
	assert_line --index 2 "  src/aws.rs:1       aws-access-token  AKIA****"
	refute_output --partial Zq3XvB7m
	refute_output --partial Q7ZR4XKD

	run --separate-stderr jouet --json secrets scan config
	assert_failure 1
	run jq -c '[.source, .files_scanned, (.findings[] | [.rule, .path, .line, .redacted, .fingerprint])]' <<<"$output"
	assert_output '["files",1,["github-pat","config/app.toml",2,"ghp_****","config/app.toml:github-pat:2"]]'

	run --separate-stderr jouet secrets scan README.md
	assert_success
	assert_output "no secrets found in 1 file (gitleaks v8.30.1, 222 rules)"
}

@test "files: ignored files are left out unless named; lockfiles and binaries are not read" {
	printf '.env\n' >.gitignore
	printf 'TOKEN=%s\n' "$(fake_github)" >.env
	printf 'integrity: %s\n' "$(fake_github)" >pnpm-lock.yaml
	printf '\0\1%s' "$(fake_github)" >blob.dat

	run --separate-stderr jouet secrets scan
	assert_success
	assert_line --index 1 "  1 binary file checked by name only"

	run --separate-stderr jouet secrets scan .env
	assert_failure 1
	assert_line --index 1 "  .env:1  github-pat  ghp_****"
}

@test "--staged: only the lines the index adds to HEAD" {
	printf 'old = "%s"\n' "$(fake_aws)" >old.txt
	git add old.txt && git commit -q -m "a key from before"
	printf 'new line\n' >>old.txt
	printf 'x\ntoken: %s\n' "$(fake_github)" >staged.txt
	git add old.txt staged.txt
	printf 'k = %s\n' "$(fake_github)" >unstaged.txt

	run --separate-stderr jouet secrets scan --staged

	assert_failure 1
	assert_line --index 0 "found 1 secret in 2 staged files (gitleaks v8.30.1, 222 rules)"
	assert_line --index 1 "  staged.txt:2  github-pat  ghp_****"
	assert_equal "${#lines[@]}" 3

	run --separate-stderr jouet secrets scan --staged old.txt
	assert_success
	assert_output "no secrets found in 1 staged file (gitleaks v8.30.1, 222 rules)"
}

@test "--diff REF: each commit's added lines, so a secret added and removed again is still found" {
	base=$(git rev-parse HEAD)
	printf 'token = "%s"\n' "$(fake_github)" >a.txt
	git add a.txt && git commit -q -m "add a token"
	added=$(git rev-parse HEAD)
	printf 'token = "from the environment"\n' >a.txt
	git commit -q -am "take it out"

	run --separate-stderr jouet secrets scan --diff "$base"
	assert_failure 1
	assert_line --index 0 "found 1 secret in 1 file in 2 commits (gitleaks v8.30.1, 222 rules)"
	assert_line --index 1 "  ${added:0:12} a.txt:1  github-pat  ghp_****"

	run --separate-stderr jouet --json secrets scan --diff "$base..$added"
	run jq -r '.commits_scanned, .findings[0].fingerprint' <<<"$output"
	assert_line --index 0 1
	assert_line --index 1 "$added:a.txt:github-pat:1"

	run --separate-stderr jouet secrets scan --diff HEAD
	assert_success

	run --separate-stderr jouet secrets scan --diff no-such-branch
	assert_failure 3
	assert_equal "$stderr" "jouet: no commit \`no-such-branch\`"
}

@test "gitleaks:allow, .gitleaksignore and a repository's .gitleaks.toml pass findings over" {
	printf 'a = "%s" # gitleaks:allow\n' "$(fake_github)" >inline.txt
	printf 'one\nb = %s\n' "$(fake_aws)" >ignored.txt
	printf '# known test data\nignored.txt:aws-access-token:2\n' >.gitleaksignore
	mkdir fixtures && printf 'c = "%s"\n' "$(fake_github)" >fixtures/token.txt
	printf '[[allowlists]]\ndescription = "test fixtures"\npaths = ['"'''"'^fixtures/'"'''"']\n' >.gitleaks.toml

	run --separate-stderr jouet --json secrets scan
	assert_success
	run jq -c '[.findings, .ignored, .config]' <<<"$output"
	assert_output '[[],1,"gitleaks v8.30.1 with .gitleaks.toml"]'

	printf '[extend]\nuseDefault = true\ndisabledRules = ["github-pat"]\n\n[[rules]]\nid = "acme-token"\nregex = '"'''"'acme_[a-z0-9]{12}'"'''"'\nkeywords = ["acme_"]\n' >.gitleaks.toml
	printf 'x = acme_%s\n' x1y2z3w4v5u6 >acme.txt
	run --separate-stderr jouet --json secrets scan
	assert_failure 1
	run jq -c '[.findings[] | [.rule, .path]]' <<<"$output"
	# github-pat is off, so the fixture's token goes unreported.
	assert_output '[["acme-token","acme.txt"]]'

	printf '[[rules]\n' >.gitleaks.toml
	run --separate-stderr jouet secrets scan
	assert_failure 4
	assert_equal "${stderr_lines[0]}" "jouet: .gitleaks.toml: TOML parse error at line 1, column 9"
}

@test "usage, missing paths and the need for a repository have their own exit codes" {
	run --separate-stderr jouet secrets scan --staged --diff HEAD
	assert_failure 2

	run --separate-stderr jouet secrets scan no-such-file
	assert_failure 3

	mkdir "$BATS_TEST_TMPDIR/plain" && cd "$BATS_TEST_TMPDIR/plain"
	run --separate-stderr jouet secrets scan --staged
	assert_failure 4
	run --separate-stderr jouet --json secrets scan --staged
	assert_equal "$(jq -r .error.code <<<"$output")" precondition
	printf 'k = %s\n' "$(fake_aws)" >k.txt
	run --separate-stderr jouet secrets scan
	assert_failure 1
	assert_line --index 1 "  k.txt:1  aws-access-token  AKIA****"
}

@test "the hook denies a git commit that would add a secret, and warns when only the generic rule matched" {
	printf 'x = %s\n' "$(fake_github)" >a.txt
	git add a.txt

	run --separate-stderr hook "git commit -m 'add a'"
	assert_success
	run jq -r '.hookSpecificOutput | .permissionDecision, .permissionDecisionReason' <<<"$output"
	assert_line --index 0 deny
	assert_line --index 1 --partial "This commit would add a secret gitleaks' rules recognise: github-pat at a.txt:1 (ghp_****)."
	refute_output --partial Zq3XvB7m

	git reset -q
	run --separate-stderr hook "git commit -m 'add a'"
	assert_output ""

	# Not staged yet: the `git add` in the same command stages it.
	run --separate-stderr hook "git add a.txt && git commit -m 'add a'"
	assert_equal "$(jq -r .hookSpecificOutput.permissionDecision <<<"$output")" deny

	rm a.txt
	fake_generic_line >b.py
	git add b.py
	run --separate-stderr hook "git commit -m 'add b'"
	run jq -r '.hookSpecificOutput | .permissionDecision // "none", .additionalContext' <<<"$output"
	assert_line --index 0 none
	assert_line --index 1 --partial "generic-api-key at b.py:1"
	assert_line --index 1 --partial "Only gitleaks' generic rule matched"
}

@test "the hook denies a push of commits that add a secret, and stays quiet once they are on a remote" {
	git init -q --bare "$BATS_TEST_TMPDIR/remote.git"
	git remote add origin "$BATS_TEST_TMPDIR/remote.git"
	git push -q -u origin main
	printf 'k = %s\n' "$(fake_aws)" >k.txt
	git add k.txt && git commit -q -m "add k"

	run --separate-stderr hook "git push"
	run jq -r '.hookSpecificOutput | .permissionDecision, .permissionDecisionReason' <<<"$output"
	assert_line --index 0 deny
	assert_line --index 1 --partial "These commits, not on any remote yet, add a secret gitleaks' rules recognise: aws-access-token at $(git rev-parse --short=12 HEAD) k.txt:1 (AKIA****)."

	run --separate-stderr hook "git push --dry-run"
	assert_output ""

	printf '%s:k.txt:aws-access-token:1\n' "$(git rev-parse HEAD)" >.gitleaksignore
	run --separate-stderr hook "git push"
	assert_output ""
}

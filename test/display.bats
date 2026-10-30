#!/usr/bin/env bats

# Who the output is for: `--display`, then `JOUET_DISPLAY`, then agent
# detection (mise-agent-env), then whether stdout is a terminal. Only human
# mode pretty-prints JSON, which makes the chosen mode visible.

setup() {
	load "test_helper/common_setup"
	_common_setup
	printf 'one\n' >a.txt
}

pretty='{
  "files": ['
minified='{"files":[{"path":"a.txt","sha256":"'

@test "piped JSON is minified unless the mode is human" {
	run --separate-stderr jouet edit hash a.txt --json

	assert_success
	assert_output --partial "$minified"
}

@test "--display human pretty-prints even when piped" {
	run --separate-stderr jouet edit hash a.txt --json --display human

	assert_success
	assert_output --partial "$pretty"
}

@test "--display wins over JOUET_DISPLAY, which wins over agent detection" {
	run --separate-stderr env JOUET_DISPLAY=human CLAUDECODE=1 jouet edit hash a.txt --json
	assert_output --partial "$pretty"

	run --separate-stderr env JOUET_DISPLAY=human jouet --display agent edit hash a.txt --json
	assert_output --partial "$minified"
}

@test "an unknown display mode is a usage error" {
	run --separate-stderr jouet edit hash a.txt --display fancy
	assert_failure 2
	assert_equal "$stderr" "jouet: --display must be agent, human or plain, not \`fancy\`"

	run --separate-stderr env JOUET_DISPLAY=fancy jouet edit hash a.txt --json
	assert_failure 2
	assert_equal "$(jq -r '.error.message' <<<"$output")" "JOUET_DISPLAY must be agent, human or plain, not \`fancy\`"
}

@test "a terminal with no agent is human" {
	run in_tty jouet edit hash a.txt --json

	assert_success
	assert_output --partial "${pretty//$'\n'/$'\r\n'}"
}

@test "CLAUDECODE=1 is an agent even on a terminal" {
	run in_tty CLAUDECODE=1 jouet edit hash a.txt --json

	assert_success
	assert_output --partial "$minified"
}

@test "AI_AGENT=0 and CLAUDECODE=0 do not count as agents" {
	run in_tty AI_AGENT=0 jouet edit hash a.txt --json
	assert_output --partial "${pretty//$'\n'/$'\r\n'}"

	run in_tty CLAUDECODE=0 jouet edit hash a.txt --json
	assert_output --partial "${pretty//$'\n'/$'\r\n'}"
}

@test "AI_AGENT=0 does not hide a specific agent marker" {
	run in_tty AI_AGENT=0 CLAUDECODE=1 jouet edit hash a.txt --json

	assert_success
	assert_output --partial "$minified"
}

@test "an agent on a terminal gets text without escape sequences" {
	run in_tty CLAUDECODE=1 jouet edit replace a.txt --old one --new 1 --diff

	assert_success
	assert_output --partial "ok          a.txt  1 edit  +1 -1"
	refute_output --partial $'\e'
}

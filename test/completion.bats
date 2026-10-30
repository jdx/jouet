#!/usr/bin/env bats

# `jouet completion`: the scripts, and the binary answering what they ask.

setup() {
	load "test_helper/common_setup"
	_common_setup
}

@test "completion prints a script for each shell that names jouet's commands" {
	for shell in bash zsh fish powershell; do
		run --separate-stderr jouet completion "$shell"
		assert_success
		[ -n "$output" ]
		assert_output --partial "jouet __complete_word__ --shell $shell"

		# What the script asks at the prompt: the subcommands come from the spec.
		run --separate-stderr jouet __complete_word__ --shell "$shell" --line 'jouet ed'
		assert_success
		assert_output --partial 'edit'
	done
}

@test "the bash script completes a subcommand" {
	jouet completion bash >"$BATS_TEST_TMPDIR/jouet.bash"
	run bash -c '
		source "$1"
		COMP_LINE="jouet wa"
		COMP_POINT=${#COMP_LINE}
		COMP_WORDS=(jouet wa)
		COMP_CWORD=1
		_usage_complete_jouet jouet wa jouet
		printf "%s\n" "${COMPREPLY[@]}"
	' _ "$BATS_TEST_TMPDIR/jouet.bash"
	assert_success
	assert_line 'wait'
}

@test "completion --json wraps the script" {
	run --separate-stderr jouet completion zsh --json
	assert_success
	[ "$(jq -r .shell <<<"$output")" = zsh ]
	jq -r .script <<<"$output" | grep -Fq '#compdef jouet'
}

@test "completion rejects an unknown shell as a usage error" {
	run --separate-stderr jouet completion bogus
	assert_failure 2
	[ -z "$output" ]
	[[ $stderr == *"bogus"* ]]
}

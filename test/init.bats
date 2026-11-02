#!/usr/bin/env bats

setup() {
	load "test_helper/common_setup"
	_common_setup
	unset XDG_CONFIG_HOME XDG_CACHE_HOME
	init_repo
}

@test "previews a rule, writes it only on request, and never trusts or overwrites it" {
	printf '[package]\nname = "example"\nversion = "0.1.0"\n' >Cargo.toml
	printf '[tasks.test]\nrun = "cargo test"\n' >mise.toml

	run --separate-stderr jouet init --json
	assert_success
	[[ ! -e .jouet ]]
	run jq -r '.proposals[0] | [.path, .exists, .written] | @tsv' <<<"$output"
	assert_output $'.jouet/rules/project-test.toml\tfalse\tfalse'

	run --separate-stderr jouet init --yes
	assert_success
	[[ -f .jouet/rules/project-test.toml ]]
	run --separate-stderr jouet rule test .jouet/rules/project-test.toml
	assert_success
	run --separate-stderr jouet rule list --json
	assert_success
	run jq -r '.rules[] | select(.name == "project-test-task") | .trust' <<<"$output"
	assert_output 'untrusted'

	printf '# user content\n' >.jouet/rules/project-test.toml
	run --separate-stderr jouet init --yes
	assert_success
	assert_line --partial 'already exists; left unchanged'
	run cat .jouet/rules/project-test.toml
	assert_output '# user content'
}

@test "unsupported projects are inventoried without inventing a rule" {
	printf '[project]\nname = "example"\n' >pyproject.toml
	printf 'test:\n\tpytest\nofflinetest: test\n\tpytest -m "not slow"\ncheck := not-a-target\n' >Makefile
	run --separate-stderr jouet init --yes --json
	assert_success
	run jq -c '[.files, [.tasks[].command], .proposals]' <<<"$output"
	assert_output '[["pyproject.toml","Makefile"],["make test","make offlinetest"],[]]'
	[[ ! -e .jouet ]]
}

@test "finds a test script without running it" {
	printf '#!/bin/sh\ntouch ran\n' >runtests.sh
	run --separate-stderr jouet init --json
	assert_success
	run jq -c '[.files, [.tasks[] | [.command, .evidence]], .proposals]' <<<"$output"
	assert_output '[["runtests.sh"],[["sh runtests.sh","root runtests.sh"]],[]]'
	[[ ! -e ran ]]
	[[ ! -e .jouet ]]
}

@test "proposes an explicitly documented pytest marker as a partial-run option" {
	printf '[project]\nname = "example"\n[tool.pytest.ini_options]\n' >pyproject.toml
	cat >Makefile <<'MAKE'
offlinetest: check
	$(PYTHON) -m pytest -Werror -m "not download"
MAKE
	run --separate-stderr jouet init --json
	assert_success
	[[ ! -e .jouet ]]
	run jq -r '.proposals[0] | [.path, .evidence, .caution, .content] | join("\n")' <<<"$output"
	assert_output --partial '.jouet/test.toml'
	assert_output --partial 'does not prove'
	assert_output --partial 'affected_args = ["-m", "not download"]'

	run --separate-stderr jouet init --yes
	assert_success
	run cat .jouet/test.toml
	assert_output $'[pytest]\naffected_args = ["-m", "not download"]'
	run --separate-stderr jouet test --affected --dry-run
	assert_failure 4
	assert_stderr --partial 'not trusted'
}

@test "does not infer pytest options from comments or a different target" {
	printf '[project]\nname = "example"\n' >pyproject.toml
	printf 'offlinetest:\n\t# pytest -m "not download"\ntest:\n\tpytest -m "not download"\n' >Makefile
	run --separate-stderr jouet init --json
	assert_success
	run jq -c '.proposals' <<<"$output"
	assert_output '[]'
}

#!/usr/bin/env bats

# `jouet review-package` writes one file a reviewer can read, or refuses a
# range that would show them the wrong diff.

setup() {
	load "test_helper/common_setup"
	_common_setup
	init_repo
	commit_file a.txt $'one\ntwo\n' "init"
	git tag base
	commit_file a.txt $'one\n2\n' "change two"
	commit_file b.md $'```sh\nls\n```\n' "add b"
}

@test "the package lands in the git directory and the summary names it" {
	run --separate-stderr jouet review-package base

	assert_success
	assert_line --index 0 --regexp '^wrote .*/\.git/jouet/review-[0-9a-f]{12}-[0-9a-f]{12}\.md$'
	assert_line --index 1 "  base..HEAD: 2 commits, 2 files changed, +4 -1"
	local file="${lines[0]#wrote }"
	grep -q '^- [0-9a-f]* change two (Test, ' "$file"
	grep -q '^````diff$' "$file"
	assert_equal "$(git status --porcelain)" ""
}

@test "-o - prints the package itself" {
	run --separate-stderr jouet review-package base HEAD -o - --context 1

	assert_success
	assert_line --index 0 "# Review: base..HEAD"
	assert_line "- diff context: 1 lines"
	assert_line "## Commits"
}

@test "swapped arguments and an empty range are refused with exit 4" {
	run --separate-stderr jouet review-package HEAD base
	assert_failure 4
	assert_equal "$stderr" "jouet: HEAD is not an ancestor of base, so the diff would include changes base does not make
hint: the two look swapped: jouet review-package base HEAD"

	run --separate-stderr jouet review-package HEAD HEAD --json
	assert_failure 4
	assert_equal "$(jq -r .error.code <<<"$output")" precondition
}

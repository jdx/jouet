#!/usr/bin/env bats
# Messages name commands in backquotes, which the tests match literally.
# shellcheck disable=SC2016

# `jouet stack status` over real worktrees, one per layer, with gh-stack's
# metadata naming the layers. GitHub is unreachable, so merged PRs are
# unknown; tests/stack.rs covers the PR chains against a fake GitHub.

setup() {
	load "test_helper/common_setup"
	_common_setup
	export JOUET_GITHUB_API_URL="http://127.0.0.1:9" JOUET_GITHUB_TOKEN=test-token JOUET_DISPLAY=plain
	export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1
	export GIT_AUTHOR_NAME=t GIT_AUTHOR_EMAIL=t@t GIT_COMMITTER_NAME=t GIT_COMMITTER_EMAIL=t@t
	git init -q -b main repo
	cd repo || return 1
	git remote add origin https://github.com/acme/app.git
	printf 'one\nbug\nthree\n' >lib.txt
	git add lib.txt
	git commit -qm init
	git branch a
	git worktree add -q ../wt-a a
	git worktree add -q -b b ../wt-b main
	cat >.git/gh-stack <<-'JSON'
		{"schemaVersion":1,"stacks":[{"trunk":{"branch":"main"},"branches":[{"branch":"a"},{"branch":"b"}]}]}
	JSON
	# Layer a fixes lib.txt; layer b is built on it in its own worktree.
	(cd ../wt-a && printf 'one\nfixed\nthree\n' >lib.txt && git commit -qam 'a: fix')
	(cd ../wt-b && git merge -q --ff-only a && echo b >b.txt && git add b.txt && git commit -qm 'b: feature')
}

@test "a stack in order passes, from any layer's worktree" {
	cd ../wt-b
	run --separate-stderr jouet stack status
	assert_success
	assert_line --index 0 'STACK main ← a ← b (from gh-stack)'
	assert_line --index 1 '  a  contains main; 1 unpushed'
	assert_line --index 2 '* b  contains a; 1 unpushed'
	assert_line --index 3 --partial 'note: GitHub could not be reached, so merged PRs are not known'
}

@test "a restack that keeps the parent's commits but reverts its fix is flagged" {
	(cd ../wt-a && printf 'one\nfixed twice\nthree\n' >lib.txt && git commit -qam 'a: fix again')
	cd ../wt-b
	run --separate-stderr jouet stack status
	assert_failure 1
	assert_line --index 2 '* b  needs a merged in; 1 unpushed'

	# Merged, but with b's tree kept: ancestry says done, content says not.
	git merge -q --no-edit -s ours a
	run --separate-stderr jouet stack status --json
	assert_failure 1
	assert_equal "$(jq -c '.layers[1] | [.contains_parent, .needs, .content[0].path, .content[0].problem]' <<<"$output")" \
		'[true,["parent_changes_missing"],"lib.txt","reverted"]'
	assert_equal "$(jq -r '.layers[1].content[0].commit' <<<"$output")" "$(git log -1 --format='%h %s' a)"

	run --separate-stderr jouet stack status
	assert_line --index 3 '    restore it with `git checkout a -- lib.txt` and commit, unless the revert is meant'

	# A real merge brings the fix in.
	git reset -q --hard HEAD^
	git merge -q --no-edit a
	run --separate-stderr jouet stack status
	assert_success
}

@test "the local branch is what counts, so unpushed fixes are not missed" {
	# The remote has an old b; the local b has the parent merged in.
	git update-ref refs/remotes/origin/main main
	git update-ref refs/remotes/origin/a a
	git update-ref refs/remotes/origin/b "$(git rev-parse b^)"
	cd ../wt-b
	run --separate-stderr jouet stack status --json
	assert_success
	assert_equal "$(jq -c '.layers[1] | [.unpushed, .contains_parent]' <<<"$output")" '[1,true]'
	assert_equal "$(jq -r '.layers[1].local' <<<"$output")" "$(git rev-parse b)"

	# The remote moved on without the local branch: the layer is behind.
	git update-ref refs/remotes/origin/b "$(git commit-tree -p b -m pushed-elsewhere 'b^{tree}')"
	run --separate-stderr jouet stack status
	assert_failure 1
	assert_line --index 2 '* b  1 behind origin/b'
}

@test "a branch gh-stack does not track is looked up on GitHub, and exits 5 without it" {
	git checkout -q -b lone
	run --separate-stderr jouet stack status
	assert_failure 5
	run --separate-stderr jouet stack status --json
	assert_equal "$(jq -r .error.code <<<"$output")" remote
}

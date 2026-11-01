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
	unset JOUET_HOOKS JOUET_HOOKS_STEER JOUET_RULES_DISABLE
	export XDG_CONFIG_HOME="$BATS_TEST_TMPDIR/config"
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

# A bare origin to fetch from and push to, beside the GitHub remote jouet
# reads the repository from.
with_origin() {
	git init -q --bare -b main ../origin.git
	git remote rename origin github
	git remote add origin ../origin.git
	git push -q origin main a b
	git fetch -q origin
}

@test "sync merges a fix up the stack, verifies it, and pushes only when asked" {
	with_origin
	(cd ../wt-a && printf 'one\nfixed twice\nthree\n' >lib.txt && git commit -qam 'a: fix again')
	git worktree remove ../wt-b
	git checkout -q --detach main

	run --separate-stderr jouet stack sync b --dry-run
	assert_success
	assert_line --index 0 'PLAN main ← a ← b (dry run: nothing fetched, merged or pushed)'
	assert_line --index 1 '  a  up to date'
	assert_line --index 2 '  b  would merge a'
	local before
	before="$(git rev-parse b)"

	run --separate-stderr jouet stack sync b
	assert_success
	assert_line --index 0 'SYNC main ← a ← b'
	assert_line --index 2 --regexp '^  b  merged a \([0-9a-f]{7}\)$'
	assert_equal "$(git show b:lib.txt)" $'one\nfixed twice\nthree'
	git merge-base --is-ancestor "$before" b
	# Merged in a temporary worktree, which is gone; nothing was pushed.
	assert_equal "$(git worktree list | wc -l | tr -d ' ')" 2
	assert_equal "$(git rev-parse origin/b)" "$before"

	run --separate-stderr jouet stack status b
	assert_success

	run --separate-stderr jouet stack sync b --push --json
	assert_success
	assert_equal "$(jq -c '[.report.pushed[] | [.branch, .ok]]' <<<"$output")" '[["a",true],["b",true]]'
	assert_equal "$(git -C ../origin.git rev-parse b)" "$(git rev-parse b)"
}

@test "sync leaves a branch checked out in another worktree to whoever works there" {
	with_origin
	(cd ../wt-a && printf 'one\nfixed twice\nthree\n' >lib.txt && git commit -qam 'a: fix again')
	local before
	before="$(git rev-parse b)"
	run --separate-stderr jouet stack sync b --no-fetch
	assert_failure 4
	assert_line --index 2 "STOPPED at b: \`b\` is checked out in $(cd ../wt-b && pwd -P), which another session may be working in"
	assert_equal "$(git rev-parse b)" "$before"

	# Run there, it merges in place.
	cd ../wt-b
	run --separate-stderr jouet stack sync --no-fetch
	assert_success
	assert_equal "$(cat lib.txt)" $'one\nfixed twice\nthree'
}

@test "sync stops at a layer whose restack reverted its parent's fix" {
	(cd ../wt-a && printf 'one\nfixed twice\nthree\n' >lib.txt && git commit -qam 'a: fix again')
	cd ../wt-b
	git merge -q --no-edit -s ours a
	run --separate-stderr jouet stack sync --no-fetch
	assert_failure 1
	assert_line --index 2 --regexp "^STOPPED at b: \`b\` would not have all of \`a\`'s changes: lib.txt \(undoes [0-9a-f]+ a: fix again\)$"
	assert_line --index 4 '  next: in `b`, restore them with `git checkout a -- lib.txt` and commit, unless the difference is meant; then rerun `jouet stack sync`'
}

# A pre-tool-use hook call for shell command $1, in this directory.
hook() {
	jq -cn --arg cwd "$PWD" --arg c "$1" \
		'{session_id: "s1", cwd: $cwd, hook_event_name: "PreToolUse", tool_name: "Bash", tool_input: {command: $c}}' |
		jouet hook claude pre-tool-use
}

@test "resetting a branch to its remote copy warns when it would drop local commits" {
	with_origin
	cd ../wt-b
	run --separate-stderr hook 'git checkout -B b origin/b'
	assert_output ''
	# A fix committed on b but not pushed.
	echo fix >fix.txt
	git add fix.txt
	git commit -qm 'b: fix'
	run --separate-stderr hook 'git checkout -B b origin/b'
	assert_equal "$(jq -r .hookSpecificOutput.additionalContext <<<"$output")" \
		'This drops 1 commit on the local `b` that `origin/b` does not have, such as fixes not pushed yet. Keep them by merging instead: `git merge origin/b` on `b`, or `jouet stack sync` for a stack of PRs. [jouet rule: reset-to-remote]'
	run --separate-stderr hook 'git reset --hard origin/b'
	assert_output --partial 'This drops 1 commit on the local `b`'
	run --separate-stderr hook 'git reset --hard HEAD^'
	assert_output ''
	# After a `cd`, the branch is the one checked out where it went.
	run --separate-stderr hook 'cd ../wt-a && git reset --hard origin/a'
	assert_output ''
	cd ../wt-a
	run --separate-stderr hook 'cd ../wt-b && git reset --hard origin/b'
	assert_output --partial 'This drops 1 commit on the local `b`'
}

@test "rebasing a layer whose prerequisite merged warns, pointing at stack sync" {
	with_origin
	cat >.git/gh-stack <<-'JSON'
		{"stacks":[{"trunk":{"branch":"main"},"branches":[{"branch":"a","pullRequest":{"number":1}},{"branch":"b","pullRequest":{"number":2}}]}]}
	JSON
	cd ../wt-b
	run --separate-stderr hook 'git rebase origin/main'
	assert_output ''
	# a merged on GitHub, which deleted its branch; the fetch pruned it.
	git push -q origin --delete a
	git fetch -q --prune origin
	run --separate-stderr hook 'git rebase origin/main'
	assert_output --partial '`b` is stacked on `a`, which has merged.'
	assert_output --partial '`jouet stack sync` does that from the local branch'
	run --separate-stderr hook 'git rebase --continue'
	assert_output ''
}

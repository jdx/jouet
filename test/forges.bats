#!/usr/bin/env bats

# GitLab and Forgejo as an agent meets them before any request is answered:
# detection from the remote, the config for a self-hosted host, and the
# features a forge lacks, refused with exit 4. The forges' APIs are an
# address nothing listens on, so a request that is made fails as remote.

setup() {
	load "test_helper/common_setup"
	_common_setup
	export JOUET_GITHUB_API_URL="http://127.0.0.1:9"
	export JOUET_GITLAB_API_URL="http://127.0.0.1:9"
	export JOUET_FORGEJO_API_URL="http://127.0.0.1:9"
	export XDG_CONFIG_HOME="$BATS_TEST_TMPDIR/config"
	export GITLAB_TOKEN="glpat-not-a-real-token"
	unset JOUET_GITLAB_TOKEN GITLAB_ACCESS_TOKEN JOUET_FORGEJO_TOKEN FORGEJO_TOKEN GITEA_TOKEN
}

@test "a GitLab remote's nested group is the repository, split at its last slash" {
	git init -q .
	git remote add origin git@gitlab.com:group/sub/project.git

	run --separate-stderr jouet pr status 12 --settle 0 --json

	assert_failure 5
	assert_equal "$(jq -r '.error.code' <<<"$output")" "remote"
	assert_output --partial "/projects/group%2Fsub%2Fproject/merge_requests/12"
}

@test "the token is never part of an error" {
	run --separate-stderr jouet pr status 'gitlab.com/group/project!12' --settle 0 --json

	assert_failure 5
	refute_output --partial "glpat-not-a-real-token"
	[[ "$stderr" != *glpat-not-a-real-token* ]]
}

@test "a host jouet does not know is refused, naming the config that fixes it" {
	run --separate-stderr jouet pr status git.example.com/group/project#3 --json

	assert_failure 4
	assert_equal "$(jq -r '.error.message' <<<"$output")" \
		"jouet does not know which forge git.example.com runs; name it in jouet's config: [forges.\"git.example.com\"] kind = \"gitlab\" (or \"forgejo\")"
}

@test "a config kind jouet cannot use is reported and left out" {
	mkdir -p "$XDG_CONFIG_HOME/jouet"
	printf '[forges."git.example.com"]\nkind = "bitbucket"\n' >"$XDG_CONFIG_HOME/jouet/config.toml"

	run --separate-stderr jouet pr status git.example.com/group/project#3

	assert_failure 4
	assert_equal "${stderr_lines[0]}" "jouet: [forges.\"git.example.com\"] kind must be gitlab or forgejo, not \`bitbucket\`"
}

@test "a configured self-hosted GitLab is read through its own API" {
	mkdir -p "$XDG_CONFIG_HOME/jouet"
	printf '[forges."git.example.com"]\nkind = "gitlab"\n' >"$XDG_CONFIG_HOME/jouet/config.toml"

	run --separate-stderr jouet pr threads 'git.example.com/group/sub/project!3' --json

	assert_failure 5
	assert_output --partial "/projects/group%2Fsub%2Fproject/merge_requests/3"
}

@test "--search on GitLab is refused: it takes GitHub's search syntax" {
	run --separate-stderr jouet pr status --search "label:bug" --repo gitlab.com/group/project --json

	assert_failure 4
	assert_output --partial "--search takes GitHub's search syntax, and gitlab.com/group/project is on GitLab"
}

@test "pr land, ci explain and wait run say what the forge lacks" {
	run --separate-stderr jouet pr land 'gitlab.com/group/project!12' --yes --json
	assert_failure 4
	assert_output --partial "glab mr merge"

	run --separate-stderr jouet ci explain 'gitlab.com/group/project!12' --json
	assert_failure 4
	assert_output --partial "GitHub Actions' run history"

	run --separate-stderr jouet wait run 7 --repo codeberg.org/forgejo/forgejo --json
	assert_failure 4
	assert_output --partial "commit statuses"
}

@test "--repo takes HOST/OWNER/NAME, and says so" {
	run --separate-stderr jouet pr threads 1 --repo "a b"

	assert_failure 2
	assert_equal "$stderr" "jouet: --repo takes OWNER/NAME or HOST/OWNER/NAME, not \`a b\`"
}

@test "a GitHub remote still wins over a GitLab mirror" {
	git init -q .
	git remote add upstream https://gitlab.com/jdx/demo.git
	git remote add origin git@github.com:jdx/demo.git
	export JOUET_GITHUB_TOKEN="test-token"

	run --separate-stderr jouet pr status 7 --settle 0 --json

	assert_failure 5
	assert_output --partial "127.0.0.1:9/graphql"
}

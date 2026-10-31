#!/usr/bin/env bats
# shellcheck disable=SC2016 # JSONPaths and ${NAME} are jouet's, not the shell's

# `jouet http` against a fake server (test/fixtures/deps/server.py, with the
# routes in test/fixtures/http/routes.json). Nothing reaches the network.

setup() {
	load "test_helper/common_setup"
	_common_setup
	command -v python3 >/dev/null || skip "python3 is not installed"
	unset HTTP_PROXY HTTPS_PROXY ALL_PROXY http_proxy https_proxy all_proxy
	python3 "$PROJECT_ROOT/test/fixtures/deps/server.py" "$PROJECT_ROOT/test/fixtures/http/routes.json" \
		"$BATS_TEST_TMPDIR/port" "$BATS_TEST_TMPDIR/requests.log" &
	SERVER=$!
	for _ in $(seq 100); do
		[[ -s "$BATS_TEST_TMPDIR/port" ]] && break
		sleep 0.05
	done
	BASE="127.0.0.1:$(cat "$BATS_TEST_TMPDIR/port")"
}

teardown() {
	[[ -n "${SERVER:-}" ]] && kill "$SERVER" 2>/dev/null || true
}

payload() {
	jq -cn --arg command "$1" --arg cwd "$PWD" \
		'{session_id: "s1", hook_event_name: "PreToolUse", tool_name: "Bash", tool_input: {command: $command}, cwd: $cwd}'
}

@test "only the selected JSONPaths are printed, after the status" {
	run --separate-stderr jouet http GET "$BASE/repos/jdx/mise" -s '$.stargazers_count' -s '$.license.spdx_id' -s '$.topics[*]'
	assert_success
	assert_line --index 0 --regexp '^HTTP 200 OK, application/json, [0-9]+ bytes, [0-9]+ ms$'
	assert_line --index 1 '$.stargazers_count: 35588'
	assert_line --index 2 '$.license.spdx_id: "MIT"'
	assert_line --index 3 '$.topics[*]: 3 matches'
	assert_line --index 4 '  "cli"'
	assert_equal "${#lines[@]}" 7
}

@test "a failed assertion exits 1 and names itself" {
	run --separate-stderr jouet http GET "$BASE/health" --expect status=200 --expect 'jsonpath $.db == "ok"'
	assert_failure 1
	assert_line --index 1 '{"status":"degraded","db":"down"}'
	assert_line --index 2 'expect failed: jsonpath $.db == "ok": got "down"'

	run --separate-stderr jouet http GET "$BASE/health" --expect 'jsonpath $.status == degraded'
	assert_success
	assert_line --index 2 'expect: 1 passed'
}

@test "an error status fails unless it is expected" {
	run --separate-stderr jouet http GET "$BASE/repos/jdx/no-such-repo" -s '$.stargazers_count'
	assert_failure 1
	assert_line --index 1 '{"message":"Not Found"}'
	assert_line --index 2 '$.stargazers_count: no match'
	assert_line --index 3 'expect failed: status < 400: got 404; give --expect status=404 to accept it'

	run --separate-stderr jouet --json http GET "$BASE/repos/jdx/no-such-repo" --expect status=404
	assert_success
	run jq -c '[.status, .body.message]' <<<"$output"
	assert_output '[404,"Not Found"]'
}

@test "in a fnox project, the values of its secrets are redacted" {
	printf '[secrets]\nDEPLOY_TOKEN = { provider = "env", value = "DEPLOY_TOKEN" }\n' >fnox.toml
	DEPLOY_TOKEN=s3cr3t-token-abc run --separate-stderr jouet http GET "$BASE/repos/jdx/mise" -s '$.note'
	assert_success
	assert_line --index 1 '$.note: "deployed with [redacted]"'
	refute_output --partial 's3cr3t'

	# Outside one, the same value is printed.
	rm fnox.toml
	DEPLOY_TOKEN=s3cr3t-token-abc run --separate-stderr jouet http GET "$BASE/repos/jdx/mise" -s '$.note'
	assert_line --index 1 '$.note: "deployed with s3cr3t-token-abc"'
}

@test "an unset \${NAME} refuses the request before anything is sent" {
	run --separate-stderr jouet http GET "$BASE/repos/jdx/mise" -H 'Authorization: Bearer ${JOUET_BATS_UNSET}'
	assert_failure 4
	assert_equal "$stderr" 'jouet: `${JOUET_BATS_UNSET}` is not set: export it, or run under `fnox exec -- jouet http ...` if fnox holds it'
	[[ ! -s "$BATS_TEST_TMPDIR/requests.log" ]]
}

@test "a routine calls it as jouet.http.request" {
	mkdir -p "$HOME/.config/jouet/routines"
	cat >"$HOME/.config/jouet/routines/stars.luau" <<'LUAU'
--- about: Count a repository's stars
--- capabilities: net
--- usage: arg "<base>" help="The API's root"
return {
	run = function(args)
		local r = jouet.http.request { method = "GET", url = args.base .. "/repos/jdx/mise", select = { "$.stargazers_count" }, expect = "status == 200" }
		return { ok = r.ok, stars = r.json.selected[1].values[1] }
	end,
}
LUAU
	run --separate-stderr jouet run stars "$BASE" --json
	assert_success
	assert_output '{"ok":true,"stars":35588}'
}

@test "curl piped into jq or Python's json warns toward jouet http; downloads and installers do not" {
	run --separate-stderr jouet hook claude pre-tool-use <<<"$(payload "curl -s $BASE/repos/jdx/mise | jq .stargazers_count")"
	assert_success
	run jq -r '.hookSpecificOutput.additionalContext' <<<"$output"
	assert_output --partial "jouet http GET URL --select '\$.path'"
	assert_output --partial '[jouet rule: curl-json]'

	run --separate-stderr jouet hook claude pre-tool-use <<<"$(payload "curl -s $BASE/x | python3 -c 'import json,sys; print(json.load(sys.stdin))'")"
	assert_output --partial '[jouet rule: curl-json]'

	for quiet in 'curl -fsSL https://mise.run | sh' "curl -o out.json $BASE/x && jq . out.json" "curl -s $BASE/x"; do
		run --separate-stderr jouet hook claude pre-tool-use <<<"$(payload "$quiet")"
		assert_output ""
	done
}

@test "agent-help lists http with wait, keeping wait's subcommands" {
	run --separate-stderr jouet agent-help
	assert_success
	assert_line --regexp '^jouet wait <ci\|run\|pr\|file\|log\|port\|http\|pid\|cmd\|job\|daemon> / jouet http: '
}

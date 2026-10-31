#!/usr/bin/env bats
# shellcheck disable=SC2016 # backticks in expected output are Markdown

# `jouet deps info` and `jouet deps audit` against a fake crates.io, npm,
# PyPI, Go proxy and OSV (test/fixtures/deps/server.py), which replays
# responses recorded from the real services and logs every request.
# Nothing reaches the network.

setup() {
	load "test_helper/common_setup"
	_common_setup
	command -v python3 >/dev/null || skip "python3 is not installed"
	mkdir .git
	export XDG_CACHE_HOME="$BATS_TEST_TMPDIR/cache"
	FIXTURES="$PROJECT_ROOT/test/fixtures/deps"
	REQUESTS="$BATS_TEST_TMPDIR/requests.log"
	: >"$REQUESTS"
	python3 "$FIXTURES/server.py" "$FIXTURES/routes.json" "$BATS_TEST_TMPDIR/port" "$REQUESTS" &
	SERVER=$!
	for _ in $(seq 100); do
		[[ -s "$BATS_TEST_TMPDIR/port" ]] && break
		sleep 0.05
	done
	JOUET_DEPS_API_URL="http://127.0.0.1:$(cat "$BATS_TEST_TMPDIR/port")"
	export JOUET_DEPS_API_URL
}

teardown() {
	[[ -n "${SERVER:-}" ]] && kill "$SERVER" 2>/dev/null || true
}

@test "an advisory on the version asked about is reported once, with its first fix" {
	run --separate-stderr jouet deps info cargo:time@0.2.7
	assert_failure 1
	assert_line --index 0 'cargo time 0.2.7 (latest 0.3.55)'
	assert_line --index 1 '  licence MIT OR Apache-2.0; repository https://github.com/time-rs/time'
	# GHSA-wcg3-cvx6-7396 and RUSTSEC-2020-0071 are the same advisory.
	assert_line --index 2 '  RUSTSEC-2020-0071 (medium): Potential segfault in the time crate; fixed in 0.2.23; also CVE-2020-26235, GHSA-wcg3-cvx6-7396'
	assert_equal "${#lines[@]}" 3
}

@test "a package that does not exist exits 3 and names only close, well-used matches" {
	run --separate-stderr jouet deps info cargo:serde-jsonx
	assert_failure 3
	assert_output 'cargo serde-jsonx: not on crates.io
  did you mean: serde_json?'
	# The search for the name found nothing, so the name less its last
	# letter was searched; core-serde-json is as close but rarely used.
	run grep -c 'q=serde-json' "$REQUESTS"
	assert_output 2
}

@test "a deprecated package and its advisory without a fix fail the check" {
	run --separate-stderr jouet deps info npm:request
	assert_failure 1
	assert_line --index 0 'npm request 2.88.2 (latest)'
	assert_line --index 1 '  deprecated: request has been deprecated, see https://github.com/request/request/issues/3142'
	assert_line --index 3 '  GHSA-p8p7-x288-28g6 (medium): Server-Side Request Forgery in Request; no fixed version; also CVE-2023-28155'
}

@test "--json prints one document for every package asked about" {
	run --separate-stderr jouet deps info npm:lodash@4.17.15 pypi:requests@2.19.0 --json
	assert_failure 1
	assert_equal "$(jq -r '.packages | length' <<<"$output")" 2
	assert_equal "$(jq -r '.packages[0] | [.ecosystem, .name, .version, .latest, .license] | join(" ")' <<<"$output")" 'npm lodash 4.17.15 4.18.1 MIT'
	assert_equal "$(jq -r '[.packages[0].advisories[] | .id + " " + .severity + " " + .fixed] | join(", ")' <<<"$output")" 'GHSA-35jh-r3h4-6jhm high 4.17.21, GHSA-p6mc-m468-83gw high 4.17.19'
	assert_equal "$(jq -r '.packages[1].repository' <<<"$output")" 'https://github.com/psf/requests'
	# OSV had nothing recorded for requests 2.19.0: no advisories, no field.
	assert_equal "$(jq -r '.packages[1] | has("advisories")' <<<"$output")" false
	# Both versions went to OSV in one batch.
	run grep -c '^POST /osv/v1/querybatch' "$REQUESTS"
	assert_output 1
}

@test "a yanked version fails, and a missing version is not found" {
	run --separate-stderr jouet deps info cargo:demo-yanked@1.0.0
	assert_failure 1
	assert_line '  yanked: breaks semver'
	run --separate-stderr jouet deps info cargo:demo-yanked@9.9.9
	assert_failure 3
	assert_output 'cargo demo-yanked 9.9.9: crates.io has no such version; the latest is 1.0.1'
}

@test "a Go module's deprecation comes from its latest go.mod" {
	run --separate-stderr jouet deps info go:github.com/golang/protobuf@1.3.0
	assert_failure 1
	assert_line --index 0 'go github.com/golang/protobuf v1.3.0 (latest v1.5.4)'
	assert_line --index 1 '  deprecated: Use the "google.golang.org/protobuf" module instead.'
	assert_line --index 3 '  no known advisories affect v1.3.0 (OSV)'
}

@test "a bare name takes the ecosystem and version its lockfile locks" {
	cat >Cargo.lock <<-'EOF'
		version = 4

		[[package]]
		name = "time"
		version = "0.2.7"
		source = "registry+https://github.com/rust-lang/crates.io-index"
	EOF
	run --separate-stderr jouet deps info time
	assert_failure 1
	assert_line --index 0 'cargo time 0.2.7 (locked in Cargo.lock; latest 0.3.55)'
	# A name no lockfile locks is taken to be in the one ecosystem here.
	run --separate-stderr jouet deps info serde-jsonx
	assert_failure 3
	assert_line --index 0 'cargo serde-jsonx: not on crates.io'
	echo '{}' >package.json
	run --separate-stderr jouet deps info serde-jsonx
	assert_failure 2
	assert_equal "$stderr" 'jouet: `serde-jsonx` could be in 2 ecosystems: this project has manifests
hint: name one: cargo:serde-jsonx or npm:serde-jsonx'
	run --separate-stderr jouet deps info maven:junit
	assert_failure 2
}

@test "a second check is answered from the cache, and --refresh asks again" {
	run jouet deps info cargo:time@0.2.7
	local before
	before=$(wc -l <"$REQUESTS")
	run jouet deps info cargo:time@0.2.7
	assert_failure 1
	assert_equal "$(wc -l <"$REQUESTS")" "$before"
	run jouet deps info cargo:time@0.2.7 --refresh
	assert_failure 1
	assert [ "$(wc -l <"$REQUESTS")" -gt "$before" ]
}

@test "audit lists only affected packages and never sends private ones" {
	cat >Cargo.lock <<-'EOF'
		version = 4

		[[package]]
		name = "app"
		version = "0.1.0"
		dependencies = ["time"]

		[[package]]
		name = "time"
		version = "0.2.7"
		source = "registry+https://github.com/rust-lang/crates.io-index"

		[[package]]
		name = "secret-crate"
		version = "1.0.0"
		source = "git+https://git.example.com/secret-crate#0123456789abcdef"
	EOF
	cat >package-lock.json <<-'EOF'
		{
		  "lockfileVersion": 3,
		  "packages": {
		    "": { "name": "app" },
		    "node_modules/lodash": { "version": "4.17.15", "resolved": "https://registry.npmjs.org/lodash/-/lodash-4.17.15.tgz" },
		    "node_modules/left-pad": { "version": "1.3.0", "resolved": "https://registry.npmjs.org/left-pad/-/left-pad-1.3.0.tgz" },
		    "node_modules/@corp/internal": { "version": "2.0.0", "resolved": "https://npm.corp.example.com/@corp/internal/-/internal-2.0.0.tgz" }
		  }
		}
	EOF
	run --separate-stderr jouet deps audit
	assert_failure 1
	assert_output '2 of 3 locked packages have advisories (OSV; Cargo.lock 2 (1 private, not sent), package-lock.json 3 (1 private, not sent))
cargo time 0.2.7 (Cargo.lock)
  RUSTSEC-2020-0071 (medium): Potential segfault in the time crate; fixed in 0.2.23; also CVE-2020-26235, GHSA-wcg3-cvx6-7396
npm lodash 4.17.15 (package-lock.json)
  GHSA-35jh-r3h4-6jhm (high): Command Injection in lodash; fixed in 4.17.21; also CVE-2021-23337
  GHSA-p6mc-m468-83gw (high): Prototype Pollution in lodash; fixed in 4.17.19; also CVE-2020-8203'
	run ! grep -E 'secret-crate|@corp/internal|"app"' "$REQUESTS"
	run --separate-stderr jouet deps audit --ecosystem npm --json
	assert_failure 1
	assert_equal "$(jq -r '[.checked, (.affected | length), .lockfiles[0].private] | join(" ")' <<<"$output")" '2 1 1'
}

@test "audit with no lockfile is not found, and one without advisories passes" {
	run --separate-stderr jouet deps audit
	assert_failure 3
	assert_equal "$stderr" "jouet: no Cargo.lock, npm lockfile, uv.lock, poetry.lock or go.mod from here to the repository's root
hint: run it in the project, or check one package with \`jouet deps info ECOSYSTEM:NAME@VERSION\`"
	cat >go.mod <<-'EOF'
		module example.com/app

		require github.com/golang/protobuf v1.3.0
	EOF
	run --separate-stderr jouet deps audit
	assert_success
	assert_output 'no known advisories in 1 locked package (OSV; go.mod 1)'
}

@test "an unreachable or failing registry exits 5" {
	JOUET_DEPS_API_URL=http://127.0.0.1:9 run --separate-stderr jouet deps info cargo:time --json
	assert_failure 5
	assert_equal "$(jq -r .error.code <<<"$output")" remote
	run --separate-stderr jouet deps info cargo:demo-down
	assert_failure 5
	assert_equal "$stderr" 'jouet: crates.io returned 503 for '"$JOUET_DEPS_API_URL"'/cargo/api/v1/crates/demo-down?include=default_version'
}

#!/usr/bin/env bats
# The hidden check for the fnox-age-identity-pr task: what the review threads
# on jdx/fnox#1099 asked of named age identities, run against the built fnox.

setup() {
	load 'test_helper/common_setup'
	_common_setup
	export XDG_RUNTIME_DIR="$TEST_TEMP_DIR/runtime"
	mkdir -p "$XDG_RUNTIME_DIR"
	unset FNOX_AGE_KEY FNOX_AGE_KEY_FILE FNOX_AGE_IDENTITY

	# Throwaway keys, made for this test and protecting nothing, so that the
	# check does not need age-keygen.
	YK1_PUBLIC=age1ta8qxljwxast8nl5k9dezaq0r4ww37tr2ea5nje3wfhfs2dthvpsmw6c6m
	YK2_PUBLIC=age1xnxpmtw5rp2qw80rukpmap6z7x2es8e5rsywvqz4ne8uahm7z3aqjnnfd2
	OTHER_PRIVATE=AGE-SECRET-KEY-19N83HS8D6854QH9Q05JQSHMLG623USGSVFAYJ728A7X7UJEZA2EQ5AVRR4 # gitleaks:allow
	echo AGE-SECRET-KEY-15JWNHVSMCUV3YGR8WV7CAMAKRZLAXRL8RZ5DA64HFN6A3KZLPVSS5T4D20 >yk1.txt # gitleaks:allow
	echo AGE-SECRET-KEY-1XR5XLXW0GGQPCEYVDZ5HM7P67QETEE2A9TVUE07KZ24MGL2MHZNSCC50RZ >yk2.txt # gitleaks:allow
}

teardown() {
	"$FNOX_BIN" daemon stop >/dev/null 2>&1 || true
	_common_teardown
}

# One provider per key, so each secret decrypts only with its own identity.
two_providers() {
	cat >fnox.toml <<EOF
root = true
$1

[providers.one]
type = "age"
recipients = ["$YK1_PUBLIC"]
default_identity = "yk1"
identities = { yk1 = "yk1.txt", yk2 = "yk2.txt" }

[providers.two]
type = "age"
recipients = ["$YK2_PUBLIC"]
default_identity = "yk1"
identities = { yk1 = "yk1.txt", yk2 = "yk2.txt" }

[secrets]
EOF
	"$FNOX_BIN" set ONE one-value --provider one
	"$FNOX_BIN" set TWO two-value --provider two
}

@test "default_identity selects a key and FNOX_AGE_IDENTITY overrides it" {
	two_providers
	run "$FNOX_BIN" get ONE
	assert_success
	assert_output "one-value"

	FNOX_AGE_IDENTITY=yk2 run "$FNOX_BIN" get TWO
	assert_success
	assert_output "two-value"
}

@test "an unknown identity name is an error that lists the available names" {
	two_providers
	FNOX_AGE_IDENTITY=nope run "$FNOX_BIN" get ONE
	assert_failure
	assert_output --partial "nope"
	assert_output --partial "yk1"
}

@test "FNOX_AGE_KEY outranks an unknown FNOX_AGE_IDENTITY" {
	two_providers
	FNOX_AGE_IDENTITY=nope FNOX_AGE_KEY="$(cat yk1.txt)" run "$FNOX_BIN" get ONE
	assert_success
	assert_output "one-value"
}

@test "a selected identity replaces the provider's identity reference" {
	cat >fnox.toml <<EOF
root = true

[providers.plain]
type = "plain"

[providers.age]
type = "age"
recipients = ["$YK1_PUBLIC"]
identity = { provider = "plain", value = "$OTHER_PRIVATE" }
default_identity = "yk1"
identities = { yk1 = "yk1.txt" }

[secrets]
EOF
	run "$FNOX_BIN" set SECRET selected-value --provider age
	assert_success
	run "$FNOX_BIN" get SECRET
	assert_success
	assert_output "selected-value"
}

@test "the daemon reads FNOX_AGE_IDENTITY on every request" {
	two_providers $'\n[daemon]\nenabled = true'
	# Non-interactive requests resolve cache misses inside the daemon.
	FNOX_AGE_IDENTITY=yk1 run "$FNOX_BIN" --non-interactive get ONE
	assert_success
	assert_output "one-value"
	FNOX_AGE_IDENTITY=yk2 run "$FNOX_BIN" --non-interactive get TWO
	assert_success
	assert_output "two-value"
}

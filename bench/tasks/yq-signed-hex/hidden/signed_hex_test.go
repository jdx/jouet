package yqlib

import "testing"

// The issue concerns JSON output from signed hexadecimal YAML integers.
func TestBenchmarkSignedHexJSON(t *testing.T) {
	for _, scenario := range []formatScenario{
		{description: "signed hex positive", input: `+0x12`, expected: "18\n", scenarioType: "encode"},
		{description: "signed hex negative", input: `-0x12`, expected: "-18\n", scenarioType: "encode"},
		{description: "signed uppercase hex", input: `+0X12`, expected: "18\n", scenarioType: "encode"},
		{description: "minimum signed hex", input: `-0x8000000000000000`, expected: "-9223372036854775808\n", scenarioType: "encode"},
	} {
		testJSONScenario(t, scenario)
	}
}

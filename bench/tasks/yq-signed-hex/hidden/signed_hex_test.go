package yqlib

import (
	"bytes"
	"strings"
	"testing"
)

// The issue concerns JSON output from signed hexadecimal YAML integers.
func TestBenchmarkSignedHexJSON(t *testing.T) {
	for _, scenario := range []struct{ input, expected string }{
		{`+0x12`, "18\n"},
		{`-0x12`, "-18\n"},
		{`+0X12`, "18\n"},
		{`-0x8000000000000000`, "-9223372036854775808\n"},
	} {
		t.Run(scenario.input, func(t *testing.T) {
			decoder := NewYamlDecoder(NewDefaultYamlPreferences())
			if err := decoder.Init(strings.NewReader(scenario.input)); err != nil {
				t.Fatal(err)
			}
			node, err := decoder.Decode()
			if err != nil {
				t.Fatal(err)
			}
			prefs := NewDefaultJsonPreferences()
			prefs.Indent, prefs.ColorsEnabled, prefs.UnwrapScalar = 0, false, false
			var output bytes.Buffer
			if err := NewJSONEncoder(prefs).Encode(&output, node); err != nil {
				t.Fatal(err)
			}
			if output.String() != scenario.expected {
				t.Fatalf("got %q, want %q", output.String(), scenario.expected)
			}
		})
	}
}

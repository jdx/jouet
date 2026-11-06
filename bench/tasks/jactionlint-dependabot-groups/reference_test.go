package jactionlint

import (
	"strings"
	"testing"
)

func TestGroupedAndRegularUpdatesAreDistinct(t *testing.T) {
	input := `version: 2
multi-ecosystem-groups:
  frontend:
    schedule:
      interval: daily
updates:
  - package-ecosystem: npm
    directory: /
    schedule:
      interval: daily
  - package-ecosystem: npm
    directory: /
    multi-ecosystem-group: frontend
    patterns: ["react*"]
`
	_, errors := ParseDependabot([]byte(input))
	for _, err := range errors {
		if strings.Contains(err.Message, "is duplicated") {
			t.Fatalf("grouped and regular updates are distinct: %s", err.Message)
		}
	}
}

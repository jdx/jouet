package main

import (
	"fmt"
	"os"
	"strings"

	"github.com/jdx/jactionlint/v2"
)

const header = `version: 2
multi-ecosystem-groups:
  frontend:
    schedule:
      interval: daily
  backend:
    schedule:
      interval: daily
updates:
`

const regular = `  - package-ecosystem: npm
    directory: /
    schedule:
      interval: daily
`

const frontend = `  - package-ecosystem: npm
    directory: /
    multi-ecosystem-group: frontend
    patterns: ["react*"]
`

const backend = `  - package-ecosystem: npm
    directory: /
    multi-ecosystem-group: backend
    patterns: ["express*"]
`

func main() {
	cases := []struct {
		name, updates string
		duplicates    int
	}{
		{"regular and group", regular + frontend, 0},
		{"two groups", frontend + backend, 0},
		{"duplicate regular", regular + regular, 1},
		{"duplicate group", frontend + frontend, 1},
	}
	failed := false
	for _, c := range cases {
		_, errors := jactionlint.ParseDependabot([]byte(header + c.updates))
		duplicates := 0
		for _, err := range errors {
			if strings.Contains(err.Message, "is duplicated") {
				duplicates++
			} else {
				fmt.Fprintf(os.Stderr, "%s: unexpected diagnostic: %s\n", c.name, err.Message)
				failed = true
			}
		}
		if duplicates != c.duplicates {
			fmt.Fprintf(os.Stderr, "%s: got %d duplicate diagnostics, want %d\n", c.name, duplicates, c.duplicates)
			failed = true
		}
	}
	if failed {
		os.Exit(1)
	}
}

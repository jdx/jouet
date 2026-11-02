package cobra_test

import (
	"bytes"
	"os"
	"reflect"
	"strings"
	"testing"

	"github.com/spf13/cobra"
)

func TestJouetCompletionKeepsArguments(t *testing.T) {
	for _, nested := range []bool{false, true} {
		t.Run(map[bool]string{false: "root", true: "child"}[nested], func(t *testing.T) {
			original := os.Args
			defer func() { os.Args = original }()
			os.Args = append(make([]string, 0, 12), "root", cobra.ShellCompNoDescRequestCmd)
			if nested {
				os.Args = append(os.Args, "child")
			}
			os.Args = append(os.Args, "al")
			want := append([]string(nil), os.Args...)
			var output bytes.Buffer
			root := &cobra.Command{Use: "root", TraverseChildren: true, Run: func(*cobra.Command, []string) {}}
			root.SetOut(&output)
			root.SetErr(&output)
			target := root
			if nested {
				target = &cobra.Command{Use: "child", Run: func(*cobra.Command, []string) {}}
				root.AddCommand(target)
			}
			called := false
			target.ValidArgsFunction = func(_ *cobra.Command, _ []string, completing string) ([]string, cobra.ShellCompDirective) {
				called = true
				if completing != "al" || !reflect.DeepEqual(os.Args, want) {
					t.Errorf("callback saw completing=%q args=%q, want al and %q", completing, os.Args, want)
				}
				return []string{"alpha"}, cobra.ShellCompDirectiveNoFileComp
			}
			if _, err := root.ExecuteC(); err != nil {
				t.Fatalf("completion failed: %v", err)
			}
			if !called || !strings.Contains(output.String(), "alpha") || !reflect.DeepEqual(os.Args, want) {
				t.Errorf("called=%v output=%q args=%q, want alpha and %q", called, output.String(), os.Args, want)
			}
		})
	}
}

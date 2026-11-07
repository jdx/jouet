// The bench's hidden test for the precedence thread (coderabbit on
// cli/src/cli/complete_word.rs): with `default_subcommand_flags`, a hidden
// `--help` on the default command must not take the spelling from the root's
// own `--help`, which the parser answers before it picks the default command.
use assert_cmd::Command;

const SPEC: &str = r#"
name "ex"
bin "ex"
default_subcommand "run"
default_subcommand_flags #true
flag "--verbose"
cmd "run" {
    flag "--help" hide=#true help="Run's own help"
    flag "--force"
}
"#;

fn offered(word: &str) -> Vec<String> {
    let out = Command::new(assert_cmd::cargo::cargo_bin!("usage"))
        .args(["cw", "--shell", "bash", "--spec", SPEC, "--", "ex", word])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    String::from_utf8(out)
        .unwrap()
        .lines()
        .map(|line| line.split('\t').next().unwrap_or("").to_string())
        .collect()
}

#[test]
fn bench_hidden_root_help_wins_over_a_hidden_default_command_help() {
    let dash = offered("-");
    assert!(dash.iter().any(|v| v == "--help"), "{dash:?}");
    assert!(dash.iter().any(|v| v == "--force"), "{dash:?}");
    let long = offered("--");
    assert!(long.iter().any(|v| v == "--help"), "{long:?}");
}

// The bench's hidden tests for the include thread (greptile on
// lib/src/spec/cmd.rs): an included file that sets a switch back to `#false`
// wins, as it did before this PR, while a mount still leaves a switch the
// command turned off alone.
#[cfg(test)]
mod bench_hidden {
    use crate::Spec;
    use std::collections::HashMap;

    #[test]
    fn bench_hidden_an_include_can_turn_the_switches_back_on() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("more.usage.kdl"),
            "disable_help_flag #false\ndisable_help_subcommand #false\ndisable_version_flag #false\n",
        )
        .unwrap();
        let main = dir.path().join("main.usage.kdl");
        std::fs::write(
            &main,
            "bin \"ex\"\nversion \"1.0.0\"\ndisable_help_flag #true\ndisable_help_subcommand #true\n\
             disable_version_flag #true\ninclude file=\"more.usage.kdl\"\n",
        )
        .unwrap();
        let spec = Spec::parse_file(&main).unwrap();
        assert!(
            !spec.cmd.disable_help_flag,
            "the include's disable_help_flag #false should win"
        );
        assert!(
            !spec.cmd.disable_help_subcommand,
            "the include's disable_help_subcommand #false should win"
        );
        assert!(
            !spec.cmd.disable_version_flag,
            "the include's disable_version_flag #false should win"
        );
    }

    #[test]
    fn bench_hidden_a_mount_keeps_a_switch_the_command_turned_off() {
        let mut spec: Spec = "bin \"ex\"\ncmd \"tasks\" disable_help_flag=#true disable_help_subcommand=#true disable_version_flag=#true {\n    mount run=\"list-tasks\"\n}\n"
            .parse()
            .unwrap();
        let outputs = HashMap::from([(
            "list-tasks".to_string(),
            "flag \"--force\"\ncmd \"build\"\n".to_string(),
        )]);
        spec.resolve_mount_outputs(&outputs).unwrap();
        let tasks = &spec.cmd.subcommands["tasks"];
        assert!(tasks.disable_help_flag, "the mount turned --help back on");
        assert!(
            tasks.disable_help_subcommand,
            "the mount turned the help subcommand back on"
        );
        assert!(
            tasks.disable_version_flag,
            "the mount turned --version back on"
        );
    }
}

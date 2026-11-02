# `jouet init`

`jouet init` inspects the current Git repository and previews project-specific
rules, including the evidence and exact proposed files. It reads project files
without running their commands. Run `jouet init --yes` after reviewing the
preview to write the proposed files; existing files are never replaced.
It also lists selected test, build, lint and check tasks found in mise and
Makefiles, and conventional Cargo commands when there is a Cargo manifest.
Inspect their definitions and project instructions before running them: their
names do not establish coverage or safety.

Currently, a Rust project with a `test` task in `mise.toml` or `.mise.toml`
gets a warning rule reminding the agent to consider that task when running
the full suite. The task may cover more than `cargo test`; the rule does not
require it on every run. Other projects are inventoried without an automatic
rule. This is a starting point, not an assessment of test coverage or the
safety of commands defined by the project.

Initialization does not trust the repository. Review the written files and
run `jouet rule test .jouet/rules/project-test.toml`; a person can then decide
whether to run `jouet trust`. Trust applies to the repository's rules,
routines, formats and plugins, not only the file created by init.

`jouet init --json` returns one document with the files and tasks found and each
proposal's path, evidence, contents and write status. No terminal is needed.

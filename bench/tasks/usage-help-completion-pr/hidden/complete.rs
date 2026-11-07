// The bench's hidden test for the views thread (greptile and coderabbit on
// argv/src/complete.rs): through an executable view of a versioned root, the
// promoted command's table does not carry `version`, but the parser still
// answers `--version` and `-V` there, so completion must offer them.
#[cfg(test)]
mod bench_hidden {
    use super::*;

    static DEPLOY: Command = Command {
        name: "deploy",
        ..Command::EMPTY
    };
    static ROOT: Command = Command {
        name: "ex",
        subcommands: &[&DEPLOY],
        version: true,
        ..Command::EMPTY
    };
    static DEPLOY_META: CommandMeta = CommandMeta {
        cmd: &DEPLOY,
        ..CommandMeta::EMPTY
    };
    static ROOT_META: CommandMeta = CommandMeta {
        cmd: &ROOT,
        subcommands: &[&DEPLOY_META],
        ..CommandMeta::EMPTY
    };
    static VIEW_SPEC: Spec = Spec {
        name: "ex",
        bin: Some("ex"),
        root: &ROOT_META,
        ..Spec::EMPTY
    };
    static DEPLOY_VIEW: crate::spec::ViewMeta = crate::spec::ViewMeta {
        id: "deploy",
        name: "deploy",
        bin: "ex-deploy",
        root: "deploy",
        all_globals: true,
        globals: &[],
    };

    fn offered_in_view(line: &str) -> Vec<String> {
        complete_view(
            &VIEW_SPEC,
            &split(line, line.len(), Shell::Bash),
            &DEPLOY_VIEW,
        )
        .candidates
        .into_iter()
        .map(|c| c.value)
        .collect()
    }

    #[test]
    fn bench_hidden_a_view_of_a_versioned_root_offers_version() {
        let dash = offered_in_view("ex-deploy -");
        for form in ["--help", "-h", "--version", "-V"] {
            assert!(
                dash.iter().any(|v| v == form),
                "{form} missing from {dash:?}"
            );
        }
        let long = offered_in_view("ex-deploy --");
        assert!(long.iter().any(|v| v == "--version"), "{long:?}");
    }

    #[test]
    fn bench_hidden_the_root_itself_offers_version_once() {
        // Not through a view: the root's table already says `version`.
        let dash: Vec<String> = candidates(&VIEW_SPEC, &split("ex -", 4, Shell::Bash))
            .into_iter()
            .map(|c| c.value)
            .collect();
        assert_eq!(
            dash.iter().filter(|v| *v == "--version").count(),
            1,
            "{dash:?}"
        );
    }
}

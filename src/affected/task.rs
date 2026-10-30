//! The repository's own test task: what `jouet test` runs for the full
//! suite, and the `test` step of a recipe.

use std::path::Path;

/// The repository's test task, by the first of these it has: a mise
/// `test` task, a package.json `test` script (run by the package manager
/// its lockfile names), Cargo.toml, a justfile or Makefile `test` target,
/// go.mod. Each runs the whole suite.
pub(crate) fn test_task(root: &Path) -> Option<Vec<String>> {
    let has = |name: &str| root.join(name).is_file();
    let read = |name: &str| std::fs::read_to_string(root.join(name)).ok();
    let words = |w: &[&str]| Some(w.iter().map(|s| (*s).to_string()).collect());
    let mise_files = [
        "mise.toml",
        ".mise.toml",
        "mise/config.toml",
        ".config/mise.toml",
        ".mise/config.toml",
    ];
    let mise_task = mise_files.iter().filter_map(|f| read(f)).any(|text| {
        toml::from_str::<toml::Table>(&text)
            .ok()
            .and_then(|t| t.get("tasks")?.as_table()?.get("test").cloned())
            .is_some()
    }) || [
        "mise-tasks/test",
        ".mise-tasks/test",
        ".mise/tasks/test",
        "mise/tasks/test",
    ]
    .iter()
    .any(|f| has(f));
    if mise_task {
        return words(&["mise", "run", "test"]);
    }
    let script = read("package.json")
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .is_some_and(|pkg| pkg["scripts"]["test"].is_string());
    if script {
        return if has("pnpm-lock.yaml") {
            words(&["pnpm", "test"])
        } else if has("yarn.lock") {
            words(&["yarn", "test"])
        } else if has("bun.lock") || has("bun.lockb") {
            words(&["bun", "run", "test"])
        } else {
            words(&["npm", "test"])
        };
    }
    if has("Cargo.toml") {
        // Every package, not only the one at the root.
        return words(&["cargo", "test", "--workspace"]);
    }
    let target = |names: &[&str]| {
        names.iter().filter_map(|f| read(f)).any(|text| {
            text.lines()
                .any(|l| l.starts_with("test:") || l.starts_with("test "))
        })
    };
    if target(&["justfile", "Justfile", ".justfile"]) {
        return words(&["just", "test"]);
    }
    if target(&["Makefile", "makefile", "GNUmakefile"]) {
        return words(&["make", "test"]);
    }
    if has("go.mod") {
        return words(&["go", "test", "./..."]);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_test_task_is_found_by_what_the_repository_has() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        assert_eq!(test_task(root), None);
        std::fs::write(root.join("go.mod"), "module x\n").unwrap();
        assert_eq!(test_task(root).unwrap(), ["go", "test", "./..."]);
        std::fs::write(
            root.join("Makefile"),
            "build:\n\ttrue\ntest: build\n\ttrue\n",
        )
        .unwrap();
        assert_eq!(test_task(root).unwrap(), ["make", "test"]);
        std::fs::write(root.join("Cargo.toml"), "[package]\n").unwrap();
        assert_eq!(test_task(root).unwrap(), ["cargo", "test", "--workspace"]);
        std::fs::write(
            root.join("package.json"),
            r#"{"scripts":{"test":"vitest"}}"#,
        )
        .unwrap();
        std::fs::write(root.join("pnpm-lock.yaml"), "").unwrap();
        assert_eq!(test_task(root).unwrap(), ["pnpm", "test"]);
        std::fs::write(root.join("mise.toml"), "[tasks.lint]\nrun = 'x'\n").unwrap();
        assert_eq!(test_task(root).unwrap(), ["pnpm", "test"]);
        std::fs::write(root.join("mise.toml"), "[tasks.test]\nrun = 'x'\n").unwrap();
        assert_eq!(test_task(root).unwrap(), ["mise", "run", "test"]);
    }
}

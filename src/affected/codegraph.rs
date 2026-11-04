//! The `graph` adapter: the fallback for ecosystems with no native one
//! (Go, Ruby, Java), selecting from jouet's code graph (`jouet_graph`): the
//! test files that reach a changed file through imports, calls, references
//! and inheritance. The graph is refreshed first, so it is current. It runs
//! the selection with each ecosystem's own runner: `go test` on the
//! selected tests' packages, rspec or minitest on their files, and Maven or
//! Gradle on their classes.

use super::{Adapter, Base, Change, Ecosystem, Error, Project, Selection, Test, executable};
use jouet_output::exit::Exit;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// The runner a project's manifest names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Runner {
    Go,
    Rspec,
    Minitest,
    Maven,
    Gradle,
}

impl Runner {
    fn of(dir: &Path) -> Option<Runner> {
        if dir.join("go.mod").is_file() {
            Some(Runner::Go)
        } else if dir.join("Gemfile").is_file() {
            let rspec = dir.join(".rspec").is_file() || dir.join("spec").is_dir();
            Some(if rspec {
                Runner::Rspec
            } else {
                Runner::Minitest
            })
        } else if dir.join("pom.xml").is_file() {
            Some(Runner::Maven)
        } else if ["build.gradle", "build.gradle.kts"]
            .iter()
            .any(|f| dir.join(f).is_file())
        {
            Some(Runner::Gradle)
        } else {
            None
        }
    }

    /// The graph language this runner's tests are written in.
    fn language(self) -> &'static str {
        match self {
            Runner::Go => "go",
            Runner::Rspec | Runner::Minitest => "ruby",
            Runner::Maven | Runner::Gradle => "java",
        }
    }
}

pub(crate) fn detect(dir: &Path) -> bool {
    Runner::of(dir).is_some()
}

pub(crate) struct Graph {
    runner: Runner,
}

impl Graph {
    pub(crate) fn load(project: &Project) -> Graph {
        Graph {
            runner: Runner::of(&project.dir).unwrap_or(Runner::Go),
        }
    }

    /// The build tool: the project's wrapper script when it has one.
    fn tool(&self, project: &Project) -> String {
        let (wrapper, plain) = match self.runner {
            Runner::Maven => ("mvnw", "mvn"),
            Runner::Gradle => ("gradlew", "gradle"),
            Runner::Go => return "go".into(),
            Runner::Rspec | Runner::Minitest => return "bundle".into(),
        };
        executable(&project.dir, wrapper)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|| plain.into())
    }
}

/// A Java test class's name, from its file: `src/test/java/a/FooTest.java`
/// is `FooTest`, which Maven's `-Dtest` and Gradle's `--tests` both take.
fn class_name(path: &str) -> &str {
    let file = path.rsplit('/').next().unwrap_or(path);
    file.strip_suffix(".java").unwrap_or(file)
}

/// Keep only go-test options that affect which source files or module are loaded.
fn go_list_flags(extra: &[String]) -> Result<Vec<String>, Error> {
    let mut flags = Vec::new();
    let mut words = extra.iter();
    while let Some(arg) = words.next() {
        if arg == "-args" || arg == "--args" {
            break;
        }
        let normalized = arg
            .strip_prefix("--")
            .map_or_else(|| arg.clone(), |rest| format!("-{rest}"));
        let name = normalized.split('=').next().unwrap_or(&normalized);
        if matches!(
            name,
            "-tags" | "-mod" | "-modfile" | "-overlay" | "-compiler"
        ) {
            let value = if let Some((_, value)) = normalized.split_once('=') {
                value.to_string()
            } else {
                words.next().cloned().ok_or_else(|| {
                    Error::new(Exit::Precondition, format!("{name} needs a value"))
                })?
            };
            if name == "-mod" && value == "mod" {
                return Err(Error::new(
                    Exit::Precondition,
                    "cannot safely validate Go packages with -mod=mod: go list may change module files",
                ));
            }
            flags.push(format!("{name}={value}"));
        } else if matches!(name, "-race" | "-msan" | "-asan") {
            flags.push(normalized);
        }
    }
    Ok(flags)
}

/// Ask Go about each selected package before producing a command. Do not fetch
/// modules or a toolchain during planning, and do not rewrite module files.
fn validate_go_packages(
    project: &Project,
    packages: &BTreeSet<String>,
    extra: &[String],
) -> Result<(), Error> {
    if packages.is_empty() {
        return Ok(());
    }
    let flags = go_list_flags(extra)?;
    let has_vendor = project
        .dir
        .ancestors()
        .any(|dir| dir.join("vendor/modules.txt").is_file());
    let vendor_flag = flags.iter().any(|flag| flag == "-mod=vendor")
        || std::env::var("GOFLAGS").is_ok_and(|value| value.contains("-mod=vendor"));
    let force_readonly = !has_vendor && !vendor_flag
        || std::env::var("GOFLAGS").is_ok_and(|value| value.contains("-mod=mod"));
    let mut argv: Vec<String> = ["go", "list", "-e", "-json"]
        .into_iter()
        .map(String::from)
        .collect();
    if force_readonly {
        argv.push("-mod=readonly".into());
    }
    argv.extend(flags);
    argv.extend(packages.iter().cloned());
    let result = crate::diag::process::run_with_env(
        &argv,
        &project.dir,
        b"",
        std::time::Duration::from_secs(30),
        &[("GOPROXY", "off"), ("GOTOOLCHAIN", "local")],
    )
    .map_err(|e| {
        Error::new(
            Exit::Precondition,
            format!("cannot validate selected Go packages: {e}"),
        )
    })?;
    if result.timed_out || result.code != Some(0) {
        let why = if result.timed_out {
            "timed out".into()
        } else {
            String::from_utf8_lossy(&result.stderr).trim().to_string()
        };
        return Err(Error::new(
            Exit::Precondition,
            format!("cannot validate selected Go packages: {why}"),
        ));
    }
    let by_dir: BTreeMap<_, _> = packages
        .iter()
        .filter_map(|package| {
            project
                .dir
                .join(package)
                .canonicalize()
                .ok()
                .map(|dir| (dir, package))
        })
        .collect();
    let mut seen = BTreeSet::new();
    let mut invalid = Vec::new();
    let mut uncertain = Vec::new();
    let values =
        serde_json::Deserializer::from_slice(&result.stdout).into_iter::<serde_json::Value>();
    for value in values {
        let value = value.map_err(|e| {
            Error::new(
                Exit::Precondition,
                format!("cannot read Go package metadata: {e}"),
            )
        })?;
        let import = value
            .get("ImportPath")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        let dir = value
            .get("Dir")
            .and_then(serde_json::Value::as_str)
            .and_then(|d| Path::new(d).canonicalize().ok());
        let package = packages
            .get(import)
            .or_else(|| dir.as_ref().and_then(|dir| by_dir.get(dir).copied()));
        let Some(package) = package else {
            return Err(Error::new(
                Exit::Precondition,
                format!("Go returned an unexpected package while validating {import}"),
            ));
        };
        seen.insert(package.clone());
        if let Some(error) = value
            .get("Error")
            .and_then(|e| e.get("Err"))
            .and_then(serde_json::Value::as_str)
        {
            invalid.push(format!("{package}: {error}"));
        } else if value
            .get("DepsErrors")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|errors| !errors.is_empty())
            || value.get("Incomplete").and_then(serde_json::Value::as_bool) == Some(true)
        {
            uncertain.push(package.clone());
        }
    }
    uncertain.extend(packages.difference(&seen).cloned());
    if !invalid.is_empty() {
        if !uncertain.is_empty() {
            invalid.push(format!(
                "could not validate dependencies for: {}",
                uncertain.join(", ")
            ));
        }
        return Err(Error::new(
            Exit::Precondition,
            format!(
                "selected Go packages cannot be loaded:\n{}",
                invalid.join("\n")
            ),
        )
        .hint("check build tags and module boundaries, then run the relevant tests"));
    }
    if !uncertain.is_empty() {
        return Err(Error::new(
            Exit::Precondition,
            format!(
                "could not validate dependencies for selected Go packages: {}",
                uncertain.join(", ")
            ),
        )
        .hint("inspect the package dependencies and ensure they and the Go toolchain are installed, then retry"));
    }
    Ok(())
}
impl Adapter for Graph {
    fn name(&self) -> &'static str {
        "graph"
    }

    fn precision(&self) -> String {
        "jouet's code graph: imports, calls, references and inheritance".into()
    }

    fn covers(&self, ecosystem: Ecosystem) -> bool {
        matches!(ecosystem, Ecosystem::Go | Ecosystem::Ruby | Ecosystem::Jvm)
    }

    fn traces(&self, path: &str) -> bool {
        jouet_graph::extract::language_for_path(path) == Some(self.runner.language())
    }

    fn traces_deletions(&self) -> bool {
        // A deleted file's dependents lose their edges to it when the graph
        // is refreshed, so they cannot be found from it.
        false
    }

    fn trigger(&self, path: &str) -> Option<&'static str> {
        let name = path.rsplit('/').next().unwrap_or(path);
        match name {
            "go.mod" => Some("go.mod changed"),
            "Gemfile" | ".rspec" => Some("the bundle or rspec configuration changed"),
            "spec_helper.rb" | "rails_helper.rb" | "test_helper.rb" => {
                Some("a test helper changed")
            }
            "pom.xml" => Some("the Maven build changed"),
            "build.gradle" | "build.gradle.kts" | "settings.gradle" | "settings.gradle.kts" => {
                Some("the Gradle build changed")
            }
            _ if path.starts_with("testdata/") || path.contains("/testdata/") => {
                Some("test data changed")
            }
            _ => None,
        }
    }

    fn full(&self, project: &Project, extra: &[String]) -> Result<Vec<String>, Error> {
        let mut argv: Vec<String> = match self.runner {
            Runner::Go => vec!["go".into(), "test".into(), "./...".into()],
            Runner::Rspec => vec!["bundle".into(), "exec".into(), "rspec".into()],
            Runner::Minitest => vec!["bundle".into(), "exec".into(), "rake".into(), "test".into()],
            Runner::Maven | Runner::Gradle => vec![self.tool(project), "test".into()],
        };
        argv.extend(extra.iter().cloned());
        Ok(argv)
    }

    fn select(
        &self,
        project: &Project,
        _base: &Base,
        sources: &[Change],
        extra: &[String],
    ) -> Result<Selection, Error> {
        let fail = |e: jouet_graph::Error| Error::new(Exit::Failed, format!("the code graph: {e}"));
        let mut graph = jouet_graph::Graph::open(&project.root).map_err(fail)?;
        graph.refresh(None).map_err(fail)?;
        if self.runner == Runner::Go
            && let Some(path) = graph
                .incomplete_files("go")
                .map_err(fail)?
                .into_iter()
                .find(|path| project.inside(path).is_some())
        {
            return Ok(Selection {
                tests: Vec::new(),
                command: self.full(project, extra)?,
                full_reason: Some(format!(
                    "the code graph could not fully parse or index {path}"
                )),
                note: None,
            });
        }
        let changed: Vec<_> = sources.iter().map(|s| project.root.join(&s.path)).collect();
        let affected = graph.affected_tests(&changed).map_err(fail)?;
        if let Some(unknown) = affected.unknown.first() {
            // A file the graph could not index (too large, not UTF-8): the
            // whole suite, rather than a selection that may miss tests.
            return Ok(Selection {
                tests: Vec::new(),
                command: self.full(project, extra)?,
                full_reason: Some(format!(
                    "the code graph could not index {}",
                    unknown.display()
                )),
                note: None,
            });
        }
        let mut tests: Vec<Test> = affected
            .tests
            .into_iter()
            .map(|t| Test {
                path: t.path.to_string_lossy().replace('\\', "/"),
                via: t
                    .via
                    .iter()
                    .map(|p| p.to_string_lossy().replace('\\', "/"))
                    .collect(),
            })
            .filter(|t| {
                project.inside(&t.path).is_some()
                    && self.traces(&t.path)
                    && (self.runner != Runner::Go || t.path.ends_with("_test.go"))
            })
            .collect();
        let mut same_package = false;
        if self.runner == Runner::Go {
            // A Go package's tests can exercise a method defined in another
            // file without a graph edge to that file (for example through an
            // interface). Run the changed package's tests even in that case.
            let changed_dirs: BTreeSet<_> = sources
                .iter()
                .filter_map(|source| Path::new(&source.path).parent())
                .collect();
            let mut selected: BTreeSet<String> =
                tests.iter().map(|test| test.path.clone()).collect();
            for dir in changed_dirs {
                let entries = std::fs::read_dir(project.root.join(dir)).map_err(|e| {
                    Error::new(
                        Exit::Failed,
                        format!("reading Go package {}: {e}", dir.display()),
                    )
                })?;
                for entry in entries {
                    let entry = entry.map_err(|e| {
                        Error::new(
                            Exit::Failed,
                            format!("reading Go package {}: {e}", dir.display()),
                        )
                    })?;
                    let name = entry.file_name().into_string().map_err(|_| {
                        Error::new(
                            Exit::Failed,
                            format!(
                                "a file in Go package {} has a non-UTF-8 name",
                                dir.display()
                            ),
                        )
                    })?;
                    if !name.ends_with("_test.go") || name.starts_with('.') || name.starts_with('_')
                    {
                        continue;
                    }
                    let metadata = std::fs::metadata(entry.path()).map_err(|e| {
                        Error::new(Exit::Failed, format!("reading Go test {name}: {e}"))
                    })?;
                    let path = dir.join(name).to_string_lossy().replace('\\', "/");
                    if metadata.is_file()
                        && project.inside(&path).is_some()
                        && selected.insert(path.clone())
                    {
                        same_package = true;
                        tests.push(Test {
                            path,
                            via: Vec::new(),
                        });
                    }
                }
            }
            tests.sort_by(|a, b| a.path.cmp(&b.path));
        }
        let mut argv: Vec<String> = match self.runner {
            Runner::Go => {
                // `go test` runs packages: the selected tests' directories.
                let packages: BTreeSet<String> = tests
                    .iter()
                    .map(|t| {
                        let dir = project
                            .relative_to_dir(&t.path)
                            .rsplit_once('/')
                            .map(|(d, _)| d.to_string());
                        dir.map_or(".".into(), |d| format!("./{d}"))
                    })
                    .collect();
                validate_go_packages(project, &packages, extra)?;
                ["go", "test"]
                    .iter()
                    .map(|w| w.to_string())
                    .chain(packages)
                    .collect()
            }
            Runner::Rspec => ["bundle", "exec", "rspec"]
                .iter()
                .map(|w| w.to_string())
                .chain(tests.iter().map(|t| project.relative_to_dir(&t.path)))
                .collect(),
            Runner::Minitest => [
                "bundle",
                "exec",
                "ruby",
                "-Itest",
                "-Ilib",
                "-e",
                "ARGV.each { |f| require File.expand_path(f) }",
            ]
            .iter()
            .map(|w| w.to_string())
            .chain(tests.iter().map(|t| project.relative_to_dir(&t.path)))
            .collect(),
            Runner::Maven => {
                let classes: Vec<&str> = tests.iter().map(|t| class_name(&t.path)).collect();
                vec![
                    self.tool(project),
                    "test".into(),
                    format!("-Dtest={}", classes.join(",")),
                    "-Dsurefire.failIfNoSpecifiedTests=false".into(),
                ]
            }
            Runner::Gradle => {
                let mut argv = vec![self.tool(project), "test".into()];
                for t in &tests {
                    argv.push("--tests".into());
                    argv.push(class_name(&t.path).into());
                }
                argv
            }
        };
        argv.extend(extra.iter().cloned());
        Ok(Selection {
            tests,
            command: argv,
            full_reason: None,
            note: same_package.then(|| "Go also runs tests in changed packages".into()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn go_package_validation_uses_build_flags_and_stops_at_test_args() {
        let args = [
            "-tags",
            "optional",
            "-race",
            "-run",
            "TestX",
            "-args",
            "-tags=ignored",
        ]
        .map(String::from);
        assert_eq!(go_list_flags(&args).unwrap(), ["-tags=optional", "-race"]);
        assert_eq!(
            go_list_flags(&["--tags=optional".into(), "-race=false".into()]).unwrap(),
            ["-tags=optional", "-race=false"]
        );
        assert!(
            go_list_flags(&["--args".into(), "-tags=ignored".into()])
                .unwrap()
                .is_empty()
        );
        assert!(go_list_flags(&["-tags".into()]).is_err());
        assert!(go_list_flags(&["-mod=mod".into()]).is_err());
    }

    #[test]
    fn go_package_validation_names_inactive_and_nested_packages() {
        if std::process::Command::new("go")
            .arg("version")
            .output()
            .is_err()
        {
            return; // Go is not required to build or test jouet itself.
        }
        let root = tempfile::tempdir().unwrap();
        for dir in ["lib", "tagged", "nested", "missing"] {
            std::fs::create_dir(root.path().join(dir)).unwrap();
        }
        std::fs::write(
            root.path().join("go.mod"),
            "module example.test/root\n\ngo 1.20\n",
        )
        .unwrap();
        std::fs::write(root.path().join("lib/lib.go"), "package lib\n").unwrap();
        std::fs::write(
            root.path().join("tagged/tagged_test.go"),
            "//go:build optional\n\npackage tagged\n",
        )
        .unwrap();
        std::fs::write(
            root.path().join("nested/go.mod"),
            "module example.test/nested\n\ngo 1.20\n",
        )
        .unwrap();
        std::fs::write(root.path().join("nested/nested.go"), "package nested\n").unwrap();
        std::fs::write(
            root.path().join("missing/missing.go"),
            "package missing\nimport _ \"example.invalid/absent\"\n",
        )
        .unwrap();
        let project = Project {
            root: root.path().into(),
            dir: root.path().into(),
            rel: String::new(),
        };
        let packages = ["./lib", "./tagged", "./nested"]
            .map(String::from)
            .into_iter()
            .collect();
        let error = validate_go_packages(&project, &packages, &[]).unwrap_err();
        assert_eq!(error.exit, Exit::Precondition);
        assert!(error.message.contains("./tagged:"), "{}", error.message);
        assert!(error.message.contains("./nested:"), "{}", error.message);
        let missing = ["./missing".to_string()].into_iter().collect();
        let error = validate_go_packages(&project, &missing, &[]).unwrap_err();
        assert!(error.message.contains("./missing"), "{}", error.message);
        assert!(
            error.message.contains("could not validate"),
            "{}",
            error.message
        );
        let tagged = ["./tagged".to_string()].into_iter().collect();
        validate_go_packages(&project, &tagged, &["-tags=optional".into()]).unwrap();
        assert!(!root.path().join("go.sum").exists());
    }
    #[test]
    fn go_package_validation_respects_automatic_vendor_mode() {
        if std::process::Command::new("go")
            .arg("version")
            .output()
            .is_err()
        {
            return;
        }
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("lib")).unwrap();
        std::fs::create_dir_all(root.path().join("vendor/example.test/dep")).unwrap();
        std::fs::write(
            root.path().join("go.mod"),
            "module example.test/root\n\ngo 1.20\n\nrequire example.test/dep v1.0.0\n",
        )
        .unwrap();
        std::fs::write(
            root.path().join("lib/lib.go"),
            "package lib\nimport _ \"example.test/dep\"\n",
        )
        .unwrap();
        std::fs::write(
            root.path().join("vendor/modules.txt"),
            "# example.test/dep v1.0.0\n## explicit; go 1.20\nexample.test/dep\n",
        )
        .unwrap();
        std::fs::write(
            root.path().join("vendor/example.test/dep/dep.go"),
            "package dep\n",
        )
        .unwrap();
        let project = Project {
            root: root.path().into(),
            dir: root.path().into(),
            rel: String::new(),
        };
        let packages = ["./lib".to_string()].into_iter().collect();
        validate_go_packages(&project, &packages, &[]).unwrap();
        assert!(!root.path().join("go.sum").exists());
    }
    #[test]
    fn java_classes_are_named_by_their_files() {
        assert_eq!(class_name("src/test/java/com/x/FooTest.java"), "FooTest");
    }

    #[test]
    fn the_runner_follows_the_manifest() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(Runner::of(dir.path()), None);
        std::fs::write(dir.path().join("Gemfile"), "").unwrap();
        assert_eq!(Runner::of(dir.path()), Some(Runner::Minitest));
        std::fs::create_dir(dir.path().join("spec")).unwrap();
        assert_eq!(Runner::of(dir.path()), Some(Runner::Rspec));
        std::fs::write(dir.path().join("go.mod"), "module x\n").unwrap();
        assert_eq!(Runner::of(dir.path()), Some(Runner::Go));
    }
}

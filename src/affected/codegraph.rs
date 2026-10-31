//! The `graph` adapter: the fallback for ecosystems with no native one
//! (Go, Ruby, Java), selecting from jouet's code graph (`jouet_graph`): the
//! test files that reach a changed file through imports, calls, references
//! and inheritance. The graph is refreshed first, so it is current. It runs
//! the selection with each ecosystem's own runner: `go test` on the
//! selected tests' packages, rspec or minitest on their files, and Maven or
//! Gradle on their classes.

use super::{Adapter, Base, Change, Ecosystem, Error, Project, Selection, Test, executable};
use jouet_output::exit::Exit;
use std::collections::BTreeSet;
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
        let changed: Vec<_> = sources.iter().map(|s| project.root.join(&s.path)).collect();
        let affected = graph.affected_tests(&changed).map_err(fail)?;
        if let Some(unknown) = affected.unknown.first() {
            // A file the graph could not index (too large, not UTF-8): the
            // whole suite, rather than a selection that may miss tests.
            return Ok(Selection {
                tests: vec![Test {
                    path: project.relative_to_root(&project.dir).unwrap_or_default(),
                    via: Vec::new(),
                }],
                command: self.full(project, extra)?,
                note: Some(format!(
                    "the code graph could not index {}, so the whole suite runs",
                    unknown.display()
                )),
            });
        }
        let tests: Vec<Test> = affected
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
            .filter(|t| project.inside(&t.path).is_some() && self.traces(&t.path))
            .collect();
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
            note: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

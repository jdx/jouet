//! The vitest and jest adapters. Both delegate the selection to the runner's
//! own module graph (`vitest list --changed`, `jest --listTests
//! --findRelatedTests`); jouet's static scan of relative imports only
//! explains each pick.

use super::classify::{extension, file_name};
use super::graph::Graph;
use super::{Adapter, Base, Change, Ecosystem, Error, Project, Selection, Test};
use jouet_output::exit::Exit;
use regex::Regex;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::LazyLock;

/// Files the runners' module graphs can reach.
const TRACED: &[&str] = &[
    "js", "jsx", "ts", "tsx", "mjs", "cjs", "mts", "cts", "vue", "svelte", "json", "css", "scss",
    "sass", "less",
];

/// Of those, the ones whose imports the static scan reads.
const CODE: &[&str] = &[
    "js", "jsx", "ts", "tsx", "mjs", "cjs", "mts", "cts", "vue", "svelte",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Runner {
    Vitest,
    Jest,
}

impl Runner {
    fn name(self) -> &'static str {
        match self {
            Self::Vitest => "vitest",
            Self::Jest => "jest",
        }
    }
}

/// package.json, when it parses.
fn manifest(dir: &Path) -> Option<serde_json::Value> {
    let text = std::fs::read_to_string(dir.join("package.json")).ok()?;
    serde_json::from_str(&text).ok()
}

/// Whether package.json depends on `name`. Scripts are not read: a
/// script may be *named* `jest` and run vitest, as renovate's does.
fn depends(pkg: &serde_json::Value, name: &str) -> bool {
    ["dependencies", "devDependencies"]
        .iter()
        .any(|key| pkg[key].get(name).is_some())
}

/// Whether `dir` has a file named `prefix` plus a JavaScript extension.
fn has_config(dir: &Path, prefixes: &[&str]) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    entries.flatten().any(|e| {
        let name = e.file_name().to_string_lossy().into_owned();
        prefixes.iter().any(|p| name.starts_with(p)) && CODE.contains(&extension(&name).as_str())
    })
}

pub(crate) fn detect_vitest(dir: &Path) -> bool {
    has_config(dir, &["vitest.config.", "vitest.workspace."])
        || manifest(dir).is_some_and(|pkg| depends(&pkg, "vitest"))
}

pub(crate) fn detect_jest(dir: &Path) -> bool {
    has_config(dir, &["jest.config."])
        || manifest(dir).is_some_and(|pkg| pkg.get("jest").is_some() || depends(&pkg, "jest"))
}

/// The vitest or jest adapter for one project.
pub(crate) struct Js {
    runner: Runner,
    /// The runner in `node_modules/.bin`, in the project or above it.
    bin: Option<PathBuf>,
}

impl Js {
    pub(crate) fn vitest(project: &Project) -> Self {
        Self::new(Runner::Vitest, project)
    }

    pub(crate) fn jest(project: &Project) -> Self {
        Self::new(Runner::Jest, project)
    }

    fn new(runner: Runner, project: &Project) -> Self {
        let bin = project
            .dir
            .ancestors()
            .take_while(|d| d.starts_with(&project.root))
            .find_map(|d| super::executable(&d.join("node_modules/.bin"), runner.name()));
        Self { runner, bin }
    }

    fn bin(&self, project: &Project) -> Result<String, Error> {
        self.bin
            .as_ref()
            .map(|b| b.to_string_lossy().into_owned())
            .ok_or_else(|| {
                Error::new(
                    Exit::Precondition,
                    format!(
                        "{} is not installed: no node_modules/.bin/{} in {} or above it",
                        self.runner.name(),
                        self.runner.name(),
                        project.dir.display()
                    ),
                )
                .hint("install the project's dependencies first, such as `npm install`")
            })
    }

    /// The test files the runner selects, by path from the root.
    fn list(
        &self,
        project: &Project,
        base: &Base,
        sources: &[Change],
    ) -> Result<Vec<String>, Error> {
        let bin = self.bin(project)?;
        let mut command = Command::new(&bin);
        let json = tempfile::NamedTempFile::new()
            .map_err(|e| Error::new(Exit::Failed, format!("creating a temporary file: {e}")))?;
        match self.runner {
            Runner::Vitest => {
                // `--json` takes an optional file; given as `--json=FILE` so
                // the list is not mixed with vitest's other output.
                command.args(["list", "--changed", &base.merge_base, "--filesOnly"]);
                command.arg(format!("--json={}", json.path().display()));
            }
            Runner::Jest => {
                command.args(["--listTests", "--findRelatedTests"]);
                command.args(sources.iter().map(|s| project.root.join(&s.path)));
            }
        }
        let out = command
            .current_dir(&project.dir)
            .stdin(Stdio::null())
            .output()
            .map_err(|e| Error::new(Exit::Precondition, format!("running {bin}: {e}")))?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            let tail: Vec<&str> = err.lines().rev().take(20).collect::<Vec<_>>();
            let tail: Vec<&str> = tail.into_iter().rev().collect();
            return Err(Error::new(
                Exit::Failed,
                format!(
                    "{} could not list the related tests (exit {}):\n{}",
                    self.runner.name(),
                    out.status.code().unwrap_or(1),
                    tail.join("\n")
                ),
            ));
        }
        let files: Vec<PathBuf> = match self.runner {
            Runner::Vitest => {
                let text = std::fs::read_to_string(json.path()).unwrap_or_default();
                vitest_files(&text)
            }
            Runner::Jest => String::from_utf8_lossy(&out.stdout)
                .lines()
                .map(str::trim)
                .filter(|l| Path::new(l).is_absolute())
                .map(PathBuf::from)
                .collect(),
        };
        let mut tests: Vec<String> = files
            .iter()
            .map(|f| {
                let f = if f.is_absolute() {
                    f.clone()
                } else {
                    project.dir.join(f)
                };
                project
                    .relative_to_root(&f)
                    .unwrap_or_else(|| f.to_string_lossy().into_owned())
            })
            .collect();
        tests.sort();
        tests.dedup();
        Ok(tests)
    }
}

/// The files in `vitest list --filesOnly --json`: objects with a `file`, or
/// plain strings.
fn vitest_files(text: &str) -> Vec<PathBuf> {
    let Ok(serde_json::Value::Array(items)) = serde_json::from_str(text) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| item.get("file").unwrap_or(item).as_str())
        .map(PathBuf::from)
        .collect()
}

impl Adapter for Js {
    fn name(&self) -> &'static str {
        self.runner.name()
    }

    fn precision(&self) -> String {
        format!("{}'s module graph", self.runner.name())
    }

    fn ecosystem(&self) -> Ecosystem {
        Ecosystem::Js
    }

    fn traces(&self, path: &str) -> bool {
        TRACED.contains(&extension(path).as_str())
    }

    fn traces_deletions(&self) -> bool {
        false
    }

    fn trigger(&self, path: &str) -> Option<&'static str> {
        let name = file_name(path);
        let lower = name.to_ascii_lowercase();
        if name == "package.json" {
            return Some("the package manifest changed");
        }
        let configs: &[&str] = match self.runner {
            Runner::Vitest => &[
                "vitest.config.",
                "vitest.workspace.",
                "vitest.projects.",
                "vite.config.",
            ],
            Runner::Jest => &["jest.config.", "jest.preset."],
        };
        let shared = [
            "tsconfig",
            "jsconfig.",
            "babel.config.",
            ".babelrc",
            ".swcrc",
            "postcss.config.",
        ];
        if configs.iter().chain(&shared).any(|p| lower.starts_with(p)) {
            return Some("the test configuration changed");
        }
        if lower.starts_with(".env") {
            return Some("an environment file changed");
        }
        let code = CODE.contains(&extension(path).as_str());
        let setup = lower.starts_with("setup")
            || ["setup.", "globalsetup", "global-setup", "global_setup"]
                .iter()
                .any(|s| lower.contains(s));
        if code && setup {
            return Some("a test setup file changed");
        }
        if path.split('/').any(|part| part == "__mocks__") {
            return Some("a manual mock changed");
        }
        None
    }

    fn full(&self, project: &Project, extra: &[String]) -> Result<Vec<String>, Error> {
        let mut argv = vec![self.bin(project)?];
        if self.runner == Runner::Vitest {
            argv.push("run".into());
        }
        argv.extend(extra.iter().cloned());
        Ok(argv)
    }

    fn select(
        &self,
        project: &Project,
        base: &Base,
        sources: &[Change],
        extra: &[String],
    ) -> Result<Selection, Error> {
        let tests = self.list(project, base, sources)?;
        let mut argv = vec![self.bin(project)?];
        match self.runner {
            Runner::Vitest => {
                let short = base.merge_base.get(..12).unwrap_or(&base.merge_base);
                argv.extend(["run".into(), "--changed".into(), short.into()]);
                argv.extend(extra.iter().cloned());
            }
            Runner::Jest => {
                argv.extend(extra.iter().cloned());
                argv.push("--findRelatedTests".into());
                argv.extend(
                    sources
                        .iter()
                        .map(|s| project.root.join(&s.path).to_string_lossy().into_owned()),
                );
            }
        }
        let changed: Vec<String> = sources.iter().map(|s| s.path.clone()).collect();
        let chains = if tests.is_empty() {
            Default::default()
        } else {
            graph(project, &changed).dependents(&changed)
        };
        let traced = tests.iter().filter(|t| chains.contains_key(*t)).count();
        let tests: Vec<Test> = tests
            .into_iter()
            .map(|path| Test {
                via: chains.get(&path).cloned().unwrap_or_default(),
                path,
            })
            .collect();
        let note = (traced < tests.len()).then(|| {
            format!(
                "{} of {} picked by {}'s module graph through imports jouet's scan does not follow (aliases, packages)",
                tests.len() - traced,
                tests.len(),
                self.runner.name()
            )
        });
        Ok(Selection {
            tests,
            command: argv,
            note,
        })
    }
}

/// The project's JavaScript and TypeScript files and their relative
/// imports, plus the changed files outside it.
fn graph(project: &Project, changed: &[String]) -> Graph {
    let mut files: BTreeSet<String> = super::changes::git(
        &project.root,
        &[
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
            "--",
            if project.rel.is_empty() {
                "."
            } else {
                &project.rel
            },
        ],
    )
    .unwrap_or_default()
    .split('\0')
    .filter(|p| CODE.contains(&extension(p).as_str()))
    .map(str::to_string)
    .collect();
    files.extend(changed.iter().cloned());
    let mut graph = Graph::default();
    for file in &files {
        if !CODE.contains(&extension(file).as_str()) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(project.root.join(file)) else {
            continue;
        };
        let imports = specifiers(&text)
            .into_iter()
            .filter_map(|spec| resolve(&project.root, file, &spec))
            .collect::<Vec<_>>();
        graph.add(file, imports);
    }
    graph
}

static IMPORT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?:\bimport\s+(?:type\s+)?(?:[\w*{}\s,$]+?\s+from\s+)?|\bexport\s+(?:type\s+)?[\w*{}\s,$]+?\s+from\s+|\brequire\s*\(\s*|\bimport\s*\(\s*|\b(?:jest|vi)\.(?:mock|requireActual|importActual)\s*\(\s*)['"]([^'"\n]+)['"]"#,
    )
    .expect("the import pattern compiles")
});

/// The module specifiers a file imports, requires or mocks.
fn specifiers(text: &str) -> Vec<String> {
    IMPORT
        .captures_iter(text)
        .filter_map(|c| c.get(1))
        .map(|m| m.as_str().to_string())
        .collect()
}

/// A relative specifier in `from` (a path from the root) as the file it
/// names; None for packages, aliases and files that do not exist.
fn resolve(root: &Path, from: &str, spec: &str) -> Option<String> {
    if !spec.starts_with("./") && !spec.starts_with("../") {
        return None;
    }
    let dir = from.rsplit_once('/').map_or("", |(d, _)| d);
    let joined = normalize(&format!("{dir}/{spec}"))?;
    let exists = |p: &str| root.join(p).is_file();
    if exists(&joined) {
        return Some(joined);
    }
    // TypeScript's ESM convention: `./x.js` names `./x.ts`.
    let swapped = [
        ("js", "ts"),
        ("js", "tsx"),
        ("mjs", "mts"),
        ("cjs", "cts"),
        ("jsx", "tsx"),
    ];
    let ext = extension(&joined);
    for (from_ext, to_ext) in swapped {
        if ext == from_ext {
            let candidate = format!("{}.{to_ext}", &joined[..joined.len() - from_ext.len() - 1]);
            if exists(&candidate) {
                return Some(candidate);
            }
        }
    }
    for ext in TRACED {
        let candidate = format!("{joined}.{ext}");
        if exists(&candidate) {
            return Some(candidate);
        }
    }
    CODE.iter()
        .map(|ext| format!("{joined}/index.{ext}"))
        .find(|p| exists(p))
}

/// `a/./b/../c` as `a/c`; None when it climbs out of the root.
fn normalize(path: &str) -> Option<String> {
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            _ => parts.push(part),
        }
    }
    Some(parts.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_every_kind_of_import() {
        let text = r#"
import { a } from './a'
import b, {
  c,
} from "../b.js"
import './side-effect'
import type { T } from './types'
export * from './reexport'
export { d } from './d'
const e = require('./e')
const f = await import('./f')
vi.mock('./g')
jest.mock("./h")
import pkg from 'some-package'
const s = "import x from './not-really'"
"#;
        let got = specifiers(text);
        for spec in [
            "./a",
            "../b.js",
            "./side-effect",
            "./types",
            "./reexport",
            "./d",
            "./e",
            "./f",
            "./g",
            "./h",
            "some-package",
        ] {
            assert!(got.contains(&spec.to_string()), "{spec} in {got:?}");
        }
    }

    #[test]
    fn resolves_relative_specifiers_like_node_and_typescript() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for file in ["src/a.ts", "src/b.js", "src/lib/index.tsx", "src/c.mts"] {
            std::fs::create_dir_all(root.join(file).parent().unwrap()).unwrap();
            std::fs::write(root.join(file), "").unwrap();
        }
        let r = |spec| resolve(root, "src/test/x.test.ts", spec);
        assert_eq!(r("../a"), Some("src/a.ts".into()));
        assert_eq!(r("../a.js"), Some("src/a.ts".into()));
        assert_eq!(r("../b.js"), Some("src/b.js".into()));
        assert_eq!(r("../lib"), Some("src/lib/index.tsx".into()));
        assert_eq!(r("../c.mjs"), Some("src/c.mts".into()));
        assert_eq!(r("../missing"), None);
        assert_eq!(r("react"), None);
        assert_eq!(r("../../../../out"), None);
    }

    #[test]
    fn config_setup_and_mocks_mean_the_whole_suite() {
        let project = Project {
            root: PathBuf::from("/nonexistent"),
            dir: PathBuf::from("/nonexistent"),
            rel: String::new(),
        };
        let vitest = Js::vitest(&project);
        for path in [
            "package.json",
            "vitest.config.ts",
            "vite.config.mjs",
            "tsconfig.json",
            "tsconfig.build.json",
            "test/setup.ts",
            "vitest.setup.ts",
            "src/__mocks__/fs.js",
            ".env.test",
        ] {
            assert!(vitest.trigger(path).is_some(), "{path}");
        }
        for path in [
            "src/app.ts",
            "test/app.test.ts",
            "jest.config.js",
            "README.md",
        ] {
            assert_eq!(vitest.trigger(path), None, "{path}");
        }
        let jest = Js::jest(&project);
        assert!(jest.trigger("jest.config.js").is_some());
        assert!(jest.trigger("src/setupTests.js").is_some());
        assert_eq!(jest.trigger("vitest.config.ts"), None);
        assert!(jest.traces("src/a.tsx") && jest.traces("data.json"));
        assert!(!jest.traces("src/a.py") && !jest.traces("logo.png"));
    }

    #[test]
    fn reads_vitest_lists_of_either_shape() {
        assert_eq!(
            vitest_files(r#"[{"file":"/r/a.test.ts"},"/r/b.test.ts"]"#),
            [PathBuf::from("/r/a.test.ts"), PathBuf::from("/r/b.test.ts")]
        );
        assert!(vitest_files("not json").is_empty());
    }

    #[test]
    fn detects_each_runner_by_its_config_or_dependency() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        assert!(!detect_vitest(d) && !detect_jest(d));
        std::fs::write(
            d.join("package.json"),
            r#"{"devDependencies":{"vitest":"5"}}"#,
        )
        .unwrap();
        assert!(detect_vitest(d) && !detect_jest(d));
        std::fs::write(
            d.join("package.json"),
            r#"{"devDependencies":{"jest":"30","jest-extended":"7"}}"#,
        )
        .unwrap();
        assert!(!detect_vitest(d) && detect_jest(d));
        std::fs::write(
            d.join("package.json"),
            r#"{"scripts":{"jest":"vitest run","test":"run-s jest"},"devDependencies":{"jest-extended":"7","vitest":"4"}}"#,
        )
        .unwrap();
        assert!(detect_vitest(d) && !detect_jest(d));
        std::fs::write(d.join("package.json"), "{}").unwrap();
        std::fs::write(d.join("vitest.config.mts"), "").unwrap();
        assert!(detect_vitest(d));
    }
}

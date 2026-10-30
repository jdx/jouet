//! The pytest adapter. With pytest-testmon installed it delegates to
//! testmon, which selects by the coverage each test recorded. Otherwise it
//! walks a static import graph from the changed modules to the test files
//! that import them, which is best-effort: dynamic imports and plugins are
//! invisible to it.

use super::classify::{extension, file_name};
use super::graph::Graph;
use super::pyimports::{self, Import};
use super::{Adapter, Base, Change, Ecosystem, Error, Project, Selection, Status, Test};
use jouet_output::exit::Exit;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Whether `dir` is a pytest project: a pytest configuration, a conftest,
/// or pytest named in its packaging.
pub(crate) fn detect(dir: &Path) -> bool {
    let has = |name: &str| dir.join(name).is_file();
    let mentions = |name: &str, needle: &str| {
        std::fs::read_to_string(dir.join(name)).is_ok_and(|text| text.contains(needle))
    };
    has("pytest.ini")
        || has("conftest.py")
        || mentions("pyproject.toml", "pytest")
        || mentions("setup.cfg", "[tool:pytest]")
        || mentions("tox.ini", "[pytest]")
        || mentions("requirements-dev.txt", "pytest")
        || mentions("requirements.txt", "pytest")
}

/// pytest's settings that change which files are tests and where imports
/// resolve, from pyproject.toml or pytest.ini.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Config {
    /// `python_files` patterns.
    files: Vec<String>,
    /// `pythonpath` entries, from the project directory.
    pythonpath: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            files: vec!["test_*.py".into(), "*_test.py".into()],
            pythonpath: Vec::new(),
        }
    }
}

impl Config {
    fn read(dir: &Path) -> Self {
        let mut config = Self::default();
        let words = |v: &toml::Value| -> Vec<String> {
            match v {
                toml::Value::String(s) => s.split_whitespace().map(str::to_string).collect(),
                toml::Value::Array(a) => a
                    .iter()
                    .filter_map(|v| v.as_str())
                    .map(str::to_string)
                    .collect(),
                _ => Vec::new(),
            }
        };
        let pyproject = std::fs::read_to_string(dir.join("pyproject.toml"))
            .ok()
            .and_then(|t| toml::from_str::<toml::Table>(&t).ok());
        let options = pyproject.as_ref().and_then(|t| {
            t.get("tool")?
                .get("pytest")?
                .get("ini_options")?
                .as_table()
                .cloned()
        });
        if let Some(options) = options {
            if let Some(v) = options.get("python_files") {
                config.files = words(v);
            }
            if let Some(v) = options.get("pythonpath") {
                config.pythonpath = words(v);
            }
            return config;
        }
        // pytest.ini: `key = value`, a value may continue on indented lines.
        let Ok(ini) = std::fs::read_to_string(dir.join("pytest.ini")) else {
            return config;
        };
        let mut key = String::new();
        let mut values: BTreeMap<String, String> = BTreeMap::new();
        for line in ini.lines() {
            if line.starts_with([' ', '\t']) && !key.is_empty() {
                let v = values.entry(key.clone()).or_default();
                v.push(' ');
                v.push_str(line.trim());
            } else if let Some((k, v)) = line.split_once('=') {
                key = k.trim().to_string();
                values.insert(key.clone(), v.trim().to_string());
            } else {
                key.clear();
            }
        }
        if let Some(v) = values.get("python_files") {
            config.files = v.split_whitespace().map(str::to_string).collect();
        }
        if let Some(v) = values.get("pythonpath") {
            config.pythonpath = v.split_whitespace().map(str::to_string).collect();
        }
        config
    }

    fn is_test(&self, path: &str) -> bool {
        let name = file_name(path);
        self.files.iter().any(|p| glob(p, name))
    }
}

/// A file-name pattern with `*` and `?`.
fn glob(pattern: &str, name: &str) -> bool {
    fn go(p: &[char], n: &[char]) -> bool {
        match (p.first(), n.first()) {
            (None, None) => true,
            (Some('*'), _) => go(&p[1..], n) || (!n.is_empty() && go(p, &n[1..])),
            (Some('?'), Some(_)) => go(&p[1..], &n[1..]),
            (Some(a), Some(b)) if a == b => go(&p[1..], &n[1..]),
            _ => false,
        }
    }
    let p: Vec<char> = pattern.chars().collect();
    let n: Vec<char> = name.chars().collect();
    go(&p, &n)
}

/// The pytest adapter for one project.
pub(crate) struct Pytest {
    /// How to start pytest: its executable, or a Python with `-m pytest`.
    pytest: Option<Vec<String>>,
    testmon: bool,
    config: Config,
}

impl Pytest {
    pub(crate) fn load(project: &Project) -> Self {
        let pytest = find_pytest(project);
        let testmon = pytest
            .as_ref()
            .and_then(|argv| python_of(argv))
            .is_some_and(|python| {
                Command::new(python)
                    .args(["-c", "import testmon"])
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .is_ok_and(|s| s.success())
            });
        Self {
            pytest,
            testmon,
            config: Config::read(&project.dir),
        }
    }

    fn pytest(&self, project: &Project) -> Result<Vec<String>, Error> {
        self.pytest.clone().ok_or_else(|| {
            Error::new(
                Exit::Precondition,
                format!(
                    "pytest is not installed: not in a .venv or venv in {}, not on PATH, and not importable by python3",
                    project.dir.display()
                ),
            )
            .hint("install the project's dependencies first, such as `uv sync` or `pip install -e .[test]`")
        })
    }

    /// testmon's selection: the test files `pytest --testmon --collect-only`
    /// lists.
    fn testmon(&self, project: &Project) -> Result<(Vec<String>, Option<String>), Error> {
        let argv = self.pytest(project)?;
        let fresh = !project.dir.join(".testmondata").exists();
        let out = Command::new(&argv[0])
            .args(&argv[1..])
            .args(["--testmon", "--collect-only", "-q"])
            .envs(self.env())
            .current_dir(&project.dir)
            .stdin(Stdio::null())
            .output()
            .map_err(|e| Error::new(Exit::Precondition, format!("running {}: {e}", argv[0])))?;
        // 5: no tests were collected, which is an answer.
        if !matches!(out.status.code(), Some(0 | 5)) {
            // pytest reports an internal error on stdout, a plugin's on stderr.
            let text = format!(
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
            let tail: Vec<&str> = text.lines().rev().take(20).collect();
            let tail: Vec<&str> = tail.into_iter().rev().collect();
            return Err(Error::new(
                Exit::Failed,
                format!(
                    "pytest --testmon could not collect the tests (exit {}):\n{}",
                    out.status.code().unwrap_or(1),
                    tail.join("\n")
                ),
            ));
        }
        let files: BTreeSet<String> = String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter_map(|l| l.split_once("::").map(|(f, _)| f.trim()))
            .map(|f| match project.relative_to_root(&project.dir.join(f)) {
                Some(p) => p,
                None => f.to_string(),
            })
            .collect();
        let note = if fresh {
            "pytest-testmon has no .testmondata yet: this run records coverage, so every test runs once"
        } else {
            "picked by pytest-testmon from the coverage each test recorded; it tracks changes since its last run"
        };
        Ok((files.into_iter().collect(), Some(note.into())))
    }

    /// The test files that import a changed module, directly or not.
    fn static_graph(&self, project: &Project, sources: &[Change]) -> Vec<Test> {
        let mut files: BTreeSet<String> = super::changes::git(
            &project.root,
            &[
                "ls-files",
                "-z",
                "--cached",
                "--others",
                "--exclude-standard",
            ],
        )
        .unwrap_or_default()
        .split('\0')
        .filter(|p| matches!(extension(p).as_str(), "py" | "pyi"))
        .map(str::to_string)
        .collect();
        let deleted: BTreeSet<&str> = sources
            .iter()
            .filter(|s| s.status == Status::Deleted)
            .map(|s| s.path.as_str())
            .collect();
        files.extend(deleted.iter().map(|p| (*p).to_string()));
        let modules = Modules::new(project, &self.config, &files);
        let mut graph = Graph::default();
        for file in &files {
            if deleted.contains(file.as_str()) {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(project.root.join(file)) else {
                continue;
            };
            let deps = pyimports::imports(&text)
                .iter()
                .flat_map(|import| modules.resolve(file, import))
                .collect::<Vec<_>>();
            graph.add(file, deps);
        }
        let changed: Vec<String> = sources.iter().map(|s| s.path.clone()).collect();
        graph
            .dependents(&changed)
            .into_iter()
            .filter(|(path, _)| {
                project.inside(path).is_some()
                    && self.config.is_test(path)
                    && !deleted.contains(path.as_str())
            })
            .map(|(path, via)| Test { path, via })
            .collect()
    }
}

/// pytest for this project: a virtualenv's in the project or the
/// repository root, the active one's, the one on PATH, or python3's module.
fn find_pytest(project: &Project) -> Option<Vec<String>> {
    let bin = if cfg!(windows) { "Scripts" } else { "bin" };
    let mut dirs: Vec<PathBuf> = Vec::new();
    for base in [&project.dir, &project.root] {
        for venv in [".venv", "venv"] {
            dirs.push(base.join(venv).join(bin));
        }
    }
    if let Some(active) = std::env::var_os("VIRTUAL_ENV") {
        dirs.push(PathBuf::from(active).join(bin));
    }
    if let Some(path) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&path));
    }
    if let Some(found) = dirs.iter().find_map(|d| super::executable(d, "pytest")) {
        return Some(vec![found.to_string_lossy().into_owned()]);
    }
    let python = if cfg!(windows) { "python" } else { "python3" };
    let importable = Command::new(python)
        .args(["-c", "import pytest"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    importable.then(|| vec![python.into(), "-m".into(), "pytest".into()])
}

/// The Python that runs a pytest command: the interpreter beside the
/// pytest executable, the one its `#!` line names, or the one given.
fn python_of(argv: &[String]) -> Option<String> {
    if argv.len() > 1 {
        return argv.first().cloned();
    }
    let pytest = Path::new(argv.first()?);
    let dir = pytest.parent()?;
    for name in ["python", "python3"] {
        if let Some(found) = super::executable(dir, name) {
            return Some(found.to_string_lossy().into_owned());
        }
    }
    let text = std::fs::read(pytest).ok()?;
    let first = text.split(|b| *b == b'\n').next()?;
    let line = String::from_utf8_lossy(first);
    let shebang = line.strip_prefix("#!")?.trim();
    shebang.split_whitespace().next().map(str::to_string)
}

/// Which file each dotted module name is, from every place an import can
/// resolve: the project, its `src`, pytest's `pythonpath`, and the first
/// directory above each file that is not a package (where pytest's
/// default import mode puts a test's directory).
struct Modules {
    files: BTreeMap<String, BTreeSet<String>>,
    /// Each file's module names, one per root it is under.
    names: BTreeMap<String, Vec<String>>,
}

impl Modules {
    fn new(project: &Project, config: &Config, files: &BTreeSet<String>) -> Self {
        let join = |dir: &str, sub: &str| {
            let joined = if dir.is_empty() {
                sub.to_string()
            } else {
                format!("{dir}/{sub}")
            };
            joined.trim_end_matches('/').to_string()
        };
        let mut roots: BTreeSet<String> = BTreeSet::new();
        roots.insert(project.rel.clone());
        roots.insert(join(&project.rel, "src"));
        for entry in &config.pythonpath {
            roots.insert(join(&project.rel, entry.trim_start_matches("./")));
        }
        let package = |dir: &str| files.contains(&join(dir, "__init__.py"));
        for file in files {
            let mut dir = file.rsplit_once('/').map_or("", |(d, _)| d);
            while !dir.is_empty() && package(dir) {
                dir = dir.rsplit_once('/').map_or("", |(d, _)| d);
            }
            roots.insert(dir.to_string());
        }
        let mut modules = Self {
            files: BTreeMap::new(),
            names: BTreeMap::new(),
        };
        for file in files {
            for root in &roots {
                let rel = if root.is_empty() {
                    Some(file.as_str())
                } else {
                    file.strip_prefix(root.as_str())
                        .and_then(|r| r.strip_prefix('/'))
                };
                let Some(rel) = rel else { continue };
                let stem = rel
                    .strip_suffix(".py")
                    .or_else(|| rel.strip_suffix(".pyi"))
                    .unwrap_or(rel);
                let stem = stem.strip_suffix("/__init__").unwrap_or(stem);
                if stem == "__init__" || stem.split('/').any(|p| p.contains('.') || p.contains('-'))
                {
                    continue;
                }
                let name = stem.replace('/', ".");
                modules
                    .files
                    .entry(name.clone())
                    .or_default()
                    .insert(file.clone());
                modules.names.entry(file.clone()).or_default().push(name);
            }
        }
        modules
    }

    /// The files an import in `from` loads: the module, each package above
    /// it (their `__init__.py` runs first), and names that are submodules.
    fn resolve(&self, from: &str, import: &Import) -> Vec<String> {
        let mut bases: Vec<String> = Vec::new();
        if import.level == 0 {
            bases.push(import.module.clone());
        } else {
            let is_package = from.ends_with("/__init__.py") || from == "__init__.py";
            for name in self.names.get(from).into_iter().flatten() {
                let mut parts: Vec<&str> = name.split('.').collect();
                // A module's package is its name without the last part; a
                // package's `__init__` is its own.
                let up = import.level - usize::from(is_package);
                if up > parts.len() {
                    continue;
                }
                parts.truncate(parts.len() - up);
                if !import.module.is_empty() {
                    parts.push(&import.module);
                }
                bases.push(parts.join("."));
            }
        }
        let mut out = Vec::new();
        for base in bases {
            let parts: Vec<&str> = base.split('.').filter(|p| !p.is_empty()).collect();
            for i in 1..=parts.len() {
                out.extend(
                    self.files
                        .get(&parts[..i].join("."))
                        .into_iter()
                        .flatten()
                        .cloned(),
                );
            }
            for name in &import.names {
                let full = if base.is_empty() {
                    name.clone()
                } else {
                    format!("{base}.{name}")
                };
                out.extend(self.files.get(&full).into_iter().flatten().cloned());
            }
        }
        out
    }
}

impl Adapter for Pytest {
    fn name(&self) -> &'static str {
        "pytest"
    }

    fn precision(&self) -> String {
        if self.testmon {
            "pytest-testmon's coverage data".into()
        } else {
            "a static import graph (best-effort: dynamic imports and plugins are invisible to it)"
                .into()
        }
    }

    fn ecosystem(&self) -> Ecosystem {
        Ecosystem::Python
    }

    fn traces(&self, path: &str) -> bool {
        matches!(extension(path).as_str(), "py" | "pyi")
    }

    fn traces_deletions(&self) -> bool {
        true
    }

    fn trigger(&self, path: &str) -> Option<&'static str> {
        match file_name(path) {
            "conftest.py" => Some("a conftest changed"),
            "pyproject.toml" | "setup.py" | "setup.cfg" => {
                Some("the package configuration changed")
            }
            "pytest.ini" | "tox.ini" | "noxfile.py" | ".coveragerc" => {
                Some("the test configuration changed")
            }
            _ => None,
        }
    }

    fn full(&self, project: &Project, extra: &[String]) -> Result<Vec<String>, Error> {
        let mut argv = self.pytest(project)?;
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
        let mut argv = self.pytest(project)?;
        argv.extend(extra.iter().cloned());
        if self.testmon {
            let (files, note) = self.testmon(project)?;
            argv.push("--testmon".into());
            return Ok(Selection {
                tests: files
                    .into_iter()
                    .map(|path| Test {
                        path,
                        via: Vec::new(),
                    })
                    .collect(),
                command: argv,
                note,
            });
        }
        let tests = self.static_graph(project, sources);
        argv.extend(tests.iter().map(|t| project.relative_to_dir(&t.path)));
        Ok(Selection {
            tests,
            command: argv,
            note: None,
        })
    }

    /// coverage's `sysmon` core, its default from Python 3.14, records no
    /// per-test contexts, so testmon misses tests that reach a change
    /// through another module. Its C tracer records them.
    fn env(&self) -> Vec<(String, String)> {
        if self.testmon && std::env::var_os("COVERAGE_CORE").is_none() {
            vec![("COVERAGE_CORE".into(), "ctrace".into())]
        } else {
            Vec::new()
        }
    }

    fn exit(&self, code: i32) -> Exit {
        match code {
            0 => Exit::Ok,
            // pytest's own usage error.
            4 => Exit::Usage,
            _ => Exit::Failed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(root: &Path, rel: &str) -> Project {
        Project {
            root: root.to_path_buf(),
            dir: root.join(rel),
            rel: rel.into(),
        }
    }

    fn write(root: &Path, files: &[(&str, &str)]) {
        for (path, text) in files {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
    }

    fn modules(root: &Path, rel: &str, config: &Config) -> Modules {
        let mut files = BTreeSet::new();
        for entry in walk(root) {
            files.insert(entry);
        }
        Modules::new(&project(root, rel), config, &files)
    }

    fn walk(dir: &Path) -> Vec<String> {
        let mut out = Vec::new();
        let mut stack = vec![dir.to_path_buf()];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().is_some_and(|x| x == "py") {
                    out.push(
                        p.strip_prefix(dir)
                            .unwrap()
                            .to_string_lossy()
                            .replace('\\', "/"),
                    );
                }
            }
        }
        out
    }

    #[test]
    fn resolves_absolute_relative_and_submodule_imports() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(
            root,
            &[
                ("app/__init__.py", ""),
                ("app/core/__init__.py", ""),
                ("app/core/math.py", ""),
                ("app/calc.py", ""),
                ("tests/helpers.py", ""),
                ("tests/test_calc.py", ""),
            ],
        );
        let m = modules(root, "", &Config::default());
        let r = |from: &str, level: usize, module: &str, names: &[&str]| {
            let mut got = m.resolve(
                from,
                &Import {
                    level,
                    module: module.into(),
                    names: names.iter().map(|n| (*n).to_string()).collect(),
                },
            );
            got.sort();
            got.dedup();
            got
        };
        assert_eq!(
            r("tests/test_calc.py", 0, "app.core.math", &[]),
            [
                "app/__init__.py",
                "app/core/__init__.py",
                "app/core/math.py"
            ]
        );
        assert_eq!(
            r("tests/test_calc.py", 0, "app.core", &["math"]),
            [
                "app/__init__.py",
                "app/core/__init__.py",
                "app/core/math.py"
            ]
        );
        // From the test's own directory, as pytest's default import mode allows.
        assert_eq!(
            r("tests/test_calc.py", 0, "helpers", &[]),
            ["tests/helpers.py"]
        );
        assert_eq!(
            r("app/calc.py", 1, "core.math", &["add"]),
            [
                "app/__init__.py",
                "app/core/__init__.py",
                "app/core/math.py"
            ]
        );
        assert_eq!(
            r("app/core/__init__.py", 1, "", &["math"]),
            [
                "app/__init__.py",
                "app/core/__init__.py",
                "app/core/math.py"
            ]
        );
        assert_eq!(r("app/calc.py", 0, "os", &["path"]), Vec::<String>::new());
    }

    #[test]
    fn src_layouts_and_pythonpath_are_roots() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(
            root,
            &[
                ("py/src/pkg/__init__.py", ""),
                ("py/src/pkg/mod.py", ""),
                ("py/lib/extra.py", ""),
            ],
        );
        let config = Config {
            pythonpath: vec!["lib".into()],
            ..Config::default()
        };
        let m = modules(root, "py", &config);
        let r = |module: &str| {
            m.resolve(
                "py/tests/test_x.py",
                &Import {
                    level: 0,
                    module: module.into(),
                    names: vec![],
                },
            )
        };
        assert!(r("pkg.mod").contains(&"py/src/pkg/mod.py".to_string()));
        assert_eq!(r("extra"), ["py/lib/extra.py"]);
    }

    #[test]
    fn reads_test_patterns_and_pythonpath() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        assert_eq!(Config::read(d), Config::default());
        std::fs::write(
            d.join("pytest.ini"),
            "[pytest]\npython_files =\n    check_*.py\n    *_spec.py\npythonpath = src\n",
        )
        .unwrap();
        let config = Config::read(d);
        assert_eq!(config.files, ["check_*.py", "*_spec.py"]);
        assert_eq!(config.pythonpath, ["src"]);
        assert!(config.is_test("tests/check_x.py") && !config.is_test("tests/test_x.py"));
        std::fs::write(
            d.join("pyproject.toml"),
            "[tool.pytest.ini_options]\npython_files = [\"t_*.py\"]\n",
        )
        .unwrap();
        assert_eq!(Config::read(d).files, ["t_*.py"]);
    }

    #[test]
    fn detects_pytest_projects() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        assert!(!detect(d));
        std::fs::write(d.join("pyproject.toml"), "[project]\nname = 'x'\n").unwrap();
        assert!(!detect(d));
        std::fs::write(
            d.join("pyproject.toml"),
            "[project.optional-dependencies]\ntest = ['pytest']\n",
        )
        .unwrap();
        assert!(detect(d));
    }

    #[test]
    fn finds_the_python_beside_pytest() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path();
        std::fs::write(bin.join("pytest"), "#!/opt/py/bin/python3.13\nimport sys\n").unwrap();
        let pytest = bin.join("pytest").to_string_lossy().into_owned();
        assert_eq!(
            python_of(std::slice::from_ref(&pytest)).as_deref(),
            Some("/opt/py/bin/python3.13")
        );
        std::fs::write(bin.join("python"), "").unwrap();
        let python = bin.join("python").to_string_lossy().into_owned();
        assert_eq!(python_of(&[pytest]).as_deref(), Some(python.as_str()));
        assert_eq!(
            python_of(&["python3".into(), "-m".into(), "pytest".into()]).as_deref(),
            Some("python3")
        );
    }
}

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

/// Only conventional test filenames: files in test directories can be helpers.
pub(crate) fn named_test(path: &str) -> bool {
    let name = file_name(path).to_ascii_lowercase();
    let Some((stem, ext)) = name.rsplit_once('.') else {
        return false;
    };
    CODE.contains(&ext)
        && [".test", ".spec", "_test", "_spec", "-test", "-spec"]
            .iter()
            .any(|suffix| stem.ends_with(suffix))
}

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
        base: Option<&Base>,
        sources: &[Change],
        extra: &[String],
    ) -> Result<Vec<String>, Error> {
        let bin = self.bin(project)?;
        let mut command = Command::new(&bin);
        let json = tempfile::NamedTempFile::new()
            .map_err(|e| Error::new(Exit::Failed, format!("creating a temporary file: {e}")))?;
        match self.runner {
            Runner::Vitest => {
                // `--json` takes an optional file; given as `--json=FILE` so
                // the list is not mixed with vitest's other output.
                command.arg("list");
                if let Some(base) = base {
                    command.args(["--changed", &base.merge_base]);
                }
                command.arg("--filesOnly");
                if base.is_some() {
                    command.args(config_args(extra));
                } else {
                    command.args(extra);
                }
                command.arg(format!("--json={}", json.path().display()));
            }
            Runner::Jest => {
                if base.is_some() {
                    command.args(config_args(extra));
                    command.args(["--listTests", "--findRelatedTests"]);
                    command.args(sources.iter().map(|s| project.root.join(&s.path)));
                } else {
                    command.args(jest_list_args(extra));
                    command.arg("--listTests");
                }
            }
        }
        command.current_dir(&project.dir).stdin(Stdio::null());
        let out = listing_output(&mut command, std::time::Duration::from_secs(120))
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

    /// The full runner's files, with the same arguments as its eventual run.
    pub(super) fn list_full(
        &self,
        project: &Project,
        extra: &[String],
    ) -> Result<Vec<String>, Error> {
        self.list(project, None, &[], extra)
    }
}

#[cfg(unix)]
static DISCOVERY_SIGNAL: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);
#[cfg(unix)]
static DISCOVERY_ACTIVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
#[cfg(unix)]
static DISCOVERY_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Signal handlers stay installed: signal-hook does not restore defaults on unregister.
#[cfg(unix)]
struct DiscoverySignals {
    _lock: std::sync::MutexGuard<'static, ()>,
}

#[cfg(unix)]
impl DiscoverySignals {
    fn install() -> std::io::Result<Self> {
        use std::sync::atomic::Ordering::SeqCst;
        static INIT: std::sync::OnceLock<Result<(), i32>> = std::sync::OnceLock::new();
        let lock = DISCOVERY_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let installed = INIT.get_or_init(|| {
            for signal in [libc::SIGINT, libc::SIGTERM] {
                // SAFETY: the handler only accesses atomics and invokes the signal-safe default handler.
                unsafe {
                    signal_hook::low_level::register(signal, move || {
                        DISCOVERY_SIGNAL.store(signal, SeqCst);
                        if !DISCOVERY_ACTIVE.load(SeqCst) {
                            let _ = signal_hook::low_level::emulate_default_handler(signal);
                        }
                    })
                }
                .map_err(|error| error.raw_os_error().unwrap_or(libc::EINVAL))?;
            }
            Ok(())
        });
        if let Err(code) = installed {
            return Err(std::io::Error::from_raw_os_error(*code));
        }
        DISCOVERY_ACTIVE.store(true, SeqCst);
        Ok(Self { _lock: lock })
    }

    fn received(&self) -> Option<i32> {
        use std::sync::atomic::Ordering::SeqCst;
        Some(DISCOVERY_SIGNAL.load(SeqCst)).filter(|signal| *signal != 0)
    }
}

#[cfg(unix)]
impl Drop for DiscoverySignals {
    fn drop(&mut self) {
        use std::sync::atomic::Ordering::SeqCst;
        DISCOVERY_ACTIVE.store(false, SeqCst);
        let signal = DISCOVERY_SIGNAL.swap(0, SeqCst);
        if signal != 0 {
            let _ = signal_hook::low_level::emulate_default_handler(signal);
        }
    }
}

/// Bound runner discovery without blocking on descendants holding output pipes.
fn listing_output(
    command: &mut Command,
    timeout: std::time::Duration,
) -> std::io::Result<std::process::Output> {
    #[cfg(unix)]
    let signals = DiscoverySignals::install()?;
    let stdout = tempfile::NamedTempFile::new()?;
    let stderr = tempfile::NamedTempFile::new()?;
    command.stdout(stdout.reopen()?).stderr(stderr.reopen()?);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command.spawn()?;
    let deadline = std::time::Instant::now() + timeout;
    let status = loop {
        #[cfg(unix)]
        if signals.received().is_some() {
            if let Ok(pid) = libc::pid_t::try_from(child.id()) {
                // SAFETY: discovery started in its own process group.
                unsafe {
                    libc::killpg(pid, libc::SIGKILL);
                }
            }
            let _ = child.kill();
            let _ = child.wait();
            drop(signals);
            unreachable!("interrupt signals terminate the process");
        }
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if std::time::Instant::now() >= deadline {
            #[cfg(unix)]
            if let Ok(pid) = libc::pid_t::try_from(child.id()) {
                // SAFETY: the child was placed in its own process group above.
                unsafe {
                    libc::killpg(pid, libc::SIGKILL);
                }
            }
            let _ = child.kill();
            let _ = child.wait();
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "test discovery timed out",
            ));
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    };
    Ok(std::process::Output {
        status,
        stdout: std::fs::read(stdout.path())?,
        stderr: std::fs::read(stderr.path())?,
    })
}

/// Preserve selection flags while keeping Jest listing output on stdout.
fn jest_list_args(extra: &[String]) -> Vec<&str> {
    let mut args = Vec::new();
    let mut iter = extra.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--outputFile" | "--output-file" => {
                iter.next();
            }
            "--json" | "--no-json" => {}
            _ if arg.starts_with("--outputFile=")
                || arg.starts_with("--output-file=")
                || arg.starts_with("--json=") => {}
            _ => args.push(arg.as_str()),
        }
    }
    args
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

    fn covers(&self, ecosystem: Ecosystem) -> bool {
        ecosystem == Ecosystem::Js
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
        let tests = self.list(project, Some(base), sources, extra)?;
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
            full_reason: None,
            note,
        })
    }
}

/// Config changes which tests the runner can see, so discovery must use the
/// same config as the eventual run. Other runner arguments may be specific to
/// execution and need not be accepted by its list command.
fn config_args(extra: &[String]) -> Vec<&str> {
    let mut args = Vec::new();
    let mut iter = extra.iter();
    while let Some(arg) = iter.next() {
        if matches!(arg.as_str(), "--config" | "-c") {
            args.push(arg.as_str());
            if let Some(value) = iter.next() {
                args.push(value.as_str());
            }
        } else if arg.starts_with("--config=") || arg.starts_with("-c=") {
            args.push(arg.as_str());
        }
    }
    args
}

/// An explicitly selected config can have any name, including one that the
/// normal filename-based classification would otherwise treat as source.
pub(crate) fn config_changed(project: &Project, extra: &[String], path: &str) -> bool {
    let args = config_args(extra);
    if args.is_empty() {
        return false;
    }
    let Ok(changed) = project.root.join(path).canonicalize() else {
        return false;
    };
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        let value = if matches!(*arg, "--config" | "-c") {
            iter.next().copied()
        } else {
            arg.split_once('=').map(|(_, value)| value)
        };
        if value.is_some_and(|value| {
            project
                .dir
                .join(value)
                .canonicalize()
                .is_ok_and(|config| config == changed)
        }) {
            return true;
        }
    }
    false
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

    #[cfg(unix)]
    #[test]
    fn listing_stops_a_hung_runner() {
        let mut command = Command::new("sh");
        command.args(["-c", "sleep 10 & wait"]);
        let started = std::time::Instant::now();
        let error = listing_output(&mut command, std::time::Duration::from_millis(50)).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
        assert!(started.elapsed() < std::time::Duration::from_secs(3));
    }

    #[cfg(unix)]
    #[test]
    fn interrupted_discovery_stops_its_process_group() {
        use std::os::unix::process::ExitStatusExt;
        use std::time::{Duration, Instant};

        if let Some(path) = std::env::var_os("JOUET_DISCOVERY_SIGNAL_TEST") {
            let mut command = Command::new("sh");
            command.args([
                "-c",
                "sleep 30 & echo $! > \"$1\"; wait",
                "_",
                &path.to_string_lossy(),
            ]);
            let _ = listing_output(&mut command, Duration::from_secs(30));
            panic!("discovery should be interrupted");
        }
        for signal in [libc::SIGINT, libc::SIGTERM] {
            let dir = tempfile::tempdir().unwrap();
            let pid_file = dir.path().join("child-pid");
            let mut process = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "affected::js::tests::interrupted_discovery_stops_its_process_group",
                ])
                .env("JOUET_DISCOVERY_SIGNAL_TEST", &pid_file)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap();
            let deadline = Instant::now() + Duration::from_secs(5);
            while !pid_file.exists() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
            assert!(pid_file.exists(), "runner did not start");
            let worker: libc::pid_t = std::fs::read_to_string(pid_file)
                .unwrap()
                .trim()
                .parse()
                .unwrap();
            // SAFETY: process is our live child and signal is a constant above.
            assert_eq!(
                unsafe { libc::kill(process.id().try_into().unwrap(), signal) },
                0
            );
            let status = process.wait().unwrap();
            assert_eq!(status.signal(), Some(signal));
            let deadline = Instant::now() + Duration::from_secs(5);
            // SAFETY: signal 0 only checks whether the recorded process exists.
            while unsafe { libc::kill(worker, 0) } == 0 && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
            // SAFETY: signal 0 only checks whether the recorded process exists.
            assert_eq!(
                unsafe { libc::kill(worker, 0) },
                -1,
                "worker survived interrupt"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn signals_still_terminate_after_discovery() {
        use std::os::unix::process::ExitStatusExt;
        use std::time::{Duration, Instant};

        if let Some(path) = std::env::var_os("JOUET_DISCOVERY_AFTER_SIGNAL_TEST") {
            let mut command = Command::new("true");
            listing_output(&mut command, Duration::from_secs(5)).unwrap();
            std::fs::write(path, "ready").unwrap();
            std::thread::sleep(Duration::from_secs(30));
            panic!("process should be interrupted");
        }
        for signal in [libc::SIGINT, libc::SIGTERM] {
            let dir = tempfile::tempdir().unwrap();
            let ready = dir.path().join("ready");
            let mut process = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "affected::js::tests::signals_still_terminate_after_discovery",
                ])
                .env("JOUET_DISCOVERY_AFTER_SIGNAL_TEST", &ready)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap();
            let deadline = Instant::now() + Duration::from_secs(5);
            while !ready.exists() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
            assert!(ready.exists(), "discovery did not finish");
            // SAFETY: process is our live child and signal is a constant above.
            assert_eq!(
                unsafe { libc::kill(process.id().try_into().unwrap(), signal) },
                0
            );
            assert_eq!(process.wait().unwrap().signal(), Some(signal));
        }
    }

    #[test]
    fn conventional_test_names_do_not_include_test_helpers() {
        for path in [
            "src/widget.test.tsx",
            "test/foo.spec.js",
            "src/foo_test.mjs",
        ] {
            assert!(named_test(path), "{path}");
        }
        for path in ["test/helpers.ts", "src/test-utils.ts", "README.md"] {
            assert!(!named_test(path), "{path}");
        }
    }

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

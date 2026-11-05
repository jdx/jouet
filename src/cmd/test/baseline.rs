//! `jouet test --baseline`: a failing run run again at the merge base, so a
//! failure that was there before the change, from the environment or an
//! earlier commit, does not count against it.

use super::{Job, execute, log_path};
use crate::testlog::{self, Format};
use crate::{affected, known};
use jouet_output::exit::Exit;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::LazyLock;

static RUST_THREAD_ID: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"^(\s*thread '[^']+') \(\d+\)( panicked at )").expect("thread ID pattern")
});
static PYTEST_SELF_ADDRESS: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"^(\s*self = <[^<>]+ object at )0x[0-9a-fA-F]+(>)$")
        .expect("pytest self address pattern")
});
/// What changes between two runs of the same Go panic: goroutine numbers,
/// a frame's argument words, and its program counter offset.
static GO_GOROUTINE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"\bgoroutine \d+\b").expect("goroutine pattern"));
static GO_FRAME_ARGS: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"^(\s*\S+\()(?:[{}\s,?.]|0x[0-9a-fA-F]+)*(\))$").expect("frame pattern")
});
static GO_FRAME_OFFSET: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"^(\s+\S+\.go:\d+) \+0x[0-9a-fA-F]+$").expect("frame offset pattern")
});
static REMARK_WARNING: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r"^\s*(\d+:\d+(?:-\d+:\d+)?)\s+warning\s+(.+\s+remark-validate-links:[\w-]+)\s*$",
    )
    .expect("remark warning pattern")
});
static REMARK_SUMMARY: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^⚠\s+(\d+) warnings?$").expect("remark summary pattern"));
static EARLIER_FAILURE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r"(?i)\b(?:error|errors|fail|failed|failure|failures|fatal|panic|panicked|broke|broken|crash|crashed|exception|traceback)\b|\bnot\s+ok\b",
    )
    .expect("earlier failure pattern")
});
static FAILURE_PREFIX: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"(?i)^(?:error|fail|failed|failure|fatal|panic|exception|traceback)\b")
        .expect("failure prefix pattern")
});
static ZERO_FAILURES: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"(?i)\b0 (?:errors?|failed|failures?)\b").expect("zero failures pattern")
});
const DETAILS_VERSION: u32 = 3;

/// What a failing run's failures were at the merge base.
#[derive(Debug, Serialize)]
pub(super) struct Baseline {
    /// The ref the merge base was taken with, and the commit.
    pub(super) base: affected::Base,
    /// The failures that also fail at the merge base.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) pre_existing: Vec<String>,
    /// The failures the merge base does not have.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) new: Vec<String>,
    /// The failures the merge base's run may not have reached: it ran far
    /// fewer tests, so a test missing from its failures may not have run.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) unknown: Vec<String>,
}

impl Baseline {
    /// Whether every failure was there at the merge base too.
    pub(super) fn none_new(&self) -> bool {
        self.new.is_empty() && self.unknown.is_empty() && !self.pre_existing.is_empty()
    }

    /// What it found, in a line or two.
    pub(super) fn text(&self) -> String {
        let at = format!(
            "{} ({})",
            self.base.name,
            &self.base.merge_base[..self.base.merge_base.len().min(12)]
        );
        let list = |names: &[String]| {
            let shown: Vec<&str> = names.iter().take(10).map(String::as_str).collect();
            let more = names.len().saturating_sub(shown.len());
            let more = if more > 0 {
                format!(" and {more} more")
            } else {
                String::new()
            };
            format!("{}{more}", shown.join(", "))
        };
        if self.none_new() {
            let all = match self.pre_existing.len() {
                1 => "the failure also fails".to_string(),
                n => format!("all {n} failures also fail"),
            };
            format!(
                "baseline: {all} at {at} with matching failure details; counted as passing: {}\n",
                list(&self.pre_existing)
            )
        } else {
            let total = self.new.len() + self.pre_existing.len() + self.unknown.len();
            let mut text = if self.new.is_empty() {
                let which = match (self.unknown.len(), total) {
                    (1, 1) => "the failure".to_string(),
                    (n, total) if n == total => format!("all {n} failures"),
                    (n, total) => format!("{n} of {total} failures"),
                };
                format!(
                    "baseline: {which} could not be compared at {at} because failure details were missing, different, ambiguous, or the run reached fewer tests: {}\n",
                    list(&self.unknown)
                )
            } else {
                let new = match (self.new.len(), total) {
                    (1, 1) => "the failure is new".to_string(),
                    (1, total) => format!("1 of {total} failures is new"),
                    (n, total) if n == total => format!("all {n} failures are new"),
                    (n, total) => format!("{n} of {total} failures are new"),
                };
                let mut text = format!(
                    "baseline: NEW FAILURE: {new} in the current run; no matching failure was reported at {at}: {}\n",
                    list(&self.new)
                );
                if !self.unknown.is_empty() {
                    text.push_str(&format!(
                        "  could not compare details or confirm the test was reached at {at}: {}\n",
                        list(&self.unknown)
                    ));
                }
                text
            };
            if !self.pre_existing.is_empty() {
                text.push_str(&format!(
                    "  also failing at {at}: {}\n",
                    list(&self.pre_existing)
                ));
            }
            text
        }
    }
}

/// Every failure a run's output names, or None when it names none, so the
/// failures cannot be compared. A path into `checkout` in a name (rustfmt's
/// `rustfmt /src/app/a.rs:3`, pytest's) is written from its root, so the same
/// failure in the merge base's worktree has the same name.
pub(super) fn failure_names(
    output: &str,
    formats: &[Format],
    checkout: &Path,
    scoped: bool,
) -> Option<BTreeSet<String>> {
    // Not as a failed run, which names an output it recognises nothing in by
    // a stand-in that would match the merge base's.
    let digest = testlog::digest_with(output, false, 0, formats);
    let names: BTreeSet<String> = digest
        .failures
        .into_iter()
        .map(|f| f.name)
        .chain(digest.more_failures)
        .map(|name| from_root(&name, checkout))
        .collect();
    if names.is_empty() {
        return None;
    }
    if scoped {
        let go = testlog::go_failure_packages(output)?;
        if !go.is_empty() {
            // Do not compare unqualified Go output, including panics or
            // build errors mixed with test failures.
            return names
                .iter()
                .all(|name| go.contains_key(name))
                .then(|| go.into_values().flatten().collect());
        }
        let jest = testlog::jest_failure_files(output)?;
        if !jest.is_empty() {
            return names.iter().all(|name| jest.contains_key(name)).then(|| {
                jest.into_values()
                    .flatten()
                    .map(|name| from_root(&name, checkout))
                    .collect()
            });
        }
    }
    Some(names)
}

/// Names and fingerprints of unambiguous, unshortened failure blocks.
pub(super) struct Evidence {
    names: BTreeSet<String>,
    details: BTreeMap<String, String>,
    /// For failures that are all Go tests: each name's packages, from `go
    /// test`'s output, so the merge base can run just those tests.
    go: Option<BTreeMap<String, BTreeSet<String>>>,
}

/// Only the complete, terminal output of remark's explicit frail link check
/// is comparable. A warning alone does not prove why a command exited nonzero.
fn remark_evidence(output: &str, root: &Path) -> Option<Evidence> {
    let lines = testlog::clean(output);
    let start = lines.iter().rposition(|line| {
        let words: Vec<_> = line.split_whitespace().collect();
        let direct = words.starts_with(&["$", "remark"]);
        let piped = words.starts_with(&["$", "xargs", "-0", "remark"])
            || words
                .windows(4)
                .any(|w| w == ["|", "xargs", "-0", "remark"]);
        line.starts_with("$ ")
            && (direct || piped)
            && words.ends_with(&[
                "remark",
                "--use",
                "remark-validate-links",
                "--frail",
                "--quiet",
            ])
    })?;
    // Unknown earlier failures cannot be attributed to the final remark
    // command. Ignore path tokens, which can contain names such as error.rs.
    if lines[..start].iter().any(|line| {
        let words = line
            .split_whitespace()
            .filter(|word| !word.contains(['/', '\\']) || FAILURE_PREFIX.is_match(word))
            .collect::<Vec<_>>()
            .join(" ");
        EARLIER_FAILURE.is_match(&ZERO_FAILURES.replace_all(&words, ""))
    }) {
        return None;
    }
    let mut file = None;
    let mut file_has_warning = false;
    let mut details = BTreeMap::new();
    let mut summary = None;
    for (offset, line) in lines[start + 1..].iter().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(c) = REMARK_SUMMARY.captures(line) {
            summary = Some((start + offset + 2, c[1].parse::<usize>().ok()?));
            break;
        }
        if line.ends_with(".md") && !line.contains(':') {
            if file.is_some() && !file_has_warning {
                return None;
            }
            file = Some(from_root(line, root));
            file_has_warning = false;
            continue;
        }
        let c = REMARK_WARNING.captures(line)?;
        let name = format!("remark {}:{}", file.as_ref()?, &c[1]);
        let detail = from_root(line, root);
        let hash = Sha256::digest(detail.as_bytes());
        let hash: String = hash.iter().map(|byte| format!("{byte:02x}")).collect();
        if details.insert(name, hash).is_some() {
            return None;
        }
        file_has_warning = true;
    }
    let (end, count) = summary?;
    let trailer: Vec<_> = lines[end..]
        .iter()
        .filter(|line| !line.trim().is_empty())
        .collect();
    if !file_has_warning
        || count == 0
        || count != details.len()
        || trailer.is_empty()
        || trailer.iter().any(|line| {
            !matches!(
                line.trim(),
                "[ELIFECYCLE] Command failed with exit code 1."
                    | "ELIFECYCLE  Command failed with exit code 1."
            )
        })
    {
        return None;
    }
    Some(Evidence {
        names: details.keys().cloned().collect(),
        details,
        go: None,
    })
}

pub(super) fn failure_evidence(
    output: &str,
    formats: &[Format],
    root: &Path,
    scoped: bool,
) -> Option<Evidence> {
    let Some(names) = failure_names(output, formats, root, scoped) else {
        return (!scoped).then(|| remark_evidence(output, root)).flatten();
    };
    let found = testlog::failure_details(output, formats);
    let matches = |name: &str, f: &testlog::FailureDetail| {
        let candidate = from_root(&f.name, root);
        if name == candidate {
            return true;
        }
        if let Some(qualified) = &f.qualified {
            return name == from_root(qualified, root);
        }
        if let Some(file) = &f.pytest_file
            && !name.starts_with(&format!("{}::", from_root(file, root)))
        {
            return false;
        }
        name.ends_with(&format!("::{candidate}"))
            || name.ends_with(&format!("::{}", candidate.replace('.', "::")))
    };
    let mut details = BTreeMap::new();
    let multiple_binaries = testlog::ambiguous_libtest(output);
    // A duplicate test name can be collapsed by the display digest. Refuse
    // all details in that case, even if one binary omitted its failure block.
    if !multiple_binaries
        || testlog::digest_with(output, false, 0, formats).failed == names.len() as u64
    {
        for name in &names {
            let candidates: Vec<_> = found.iter().filter(|f| matches(name, f)).collect();
            if let [failure] = candidates.as_slice()
                && (!multiple_binaries || failure.binary.is_some())
                && names.iter().filter(|other| matches(other, failure)).count() == 1
            {
                let lines: Vec<_> = failure
                    .lines
                    .iter()
                    .map(|line| {
                        let line = from_root(line, root);
                        let line = RUST_THREAD_ID.replace(&line, "$1$2");
                        let line = GO_GOROUTINE.replace_all(&line, "goroutine N");
                        let line = GO_FRAME_ARGS.replace(&line, "$1...$2");
                        let line = GO_FRAME_OFFSET.replace(&line, "$1");
                        PYTEST_SELF_ADDRESS
                            .replace(&line, "${1}<address>${2}")
                            .into_owned()
                    })
                    .collect();
                let mut hash = Sha256::new();
                if let Some(binary) = &failure.binary {
                    let binary = from_root(binary, root);
                    hash.update((binary.len() as u64).to_be_bytes());
                    hash.update(binary.as_bytes());
                }
                for line in lines {
                    hash.update((line.len() as u64).to_be_bytes());
                    hash.update(line.as_bytes());
                }
                details.insert(
                    name.clone(),
                    hash.finalize()
                        .iter()
                        .map(|byte| format!("{byte:02x}"))
                        .collect(),
                );
            }
        }
    }
    let go = testlog::go_failure_packages(output)
        .filter(|go| !go.is_empty() && names.iter().all(|name| go.contains_key(name)));
    Some(Evidence { names, details, go })
}

/// The merge base of HEAD with `since` (the upstream or default branch when
/// None), checked before the run so a comparison that cannot tell anything
/// costs nothing: when the work tree is that commit exactly, as after
/// committing on the default branch itself, the run there would be the
/// same code, and every failure would look as if it were there before.
pub(super) fn base(root: &Path, since: Option<&str>) -> Result<affected::Base, affected::Error> {
    let base = affected::base(root, since)?;
    if affected::unchanged_since(root, &base.merge_base)? {
        return Err(affected::Error::new(
            Exit::Usage,
            format!(
                "--baseline compares with the merge base of HEAD and `{}`, which is HEAD itself with nothing changed since, so it would run this same code: name the commit your change started from with --since",
                base.name
            ),
        ));
    }
    Ok(base)
}

/// Whether [`compare`] may run a failed job at the merge base when nothing
/// is kept for it ([`known`]), retry an inconclusive run, or only use what is kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Mode {
    Run,
    /// An affected selection must be compared with this invocation's environment.
    Fresh,
    /// An explicit retry may replace an inconclusive cached run.
    Retry,
    Kept,
}

/// Why the failures could not be compared. `unverifiable` when the merge
/// base could not run the command either, so nothing ever will be here:
/// the stop gate then asks about the failures once, not at every turn.
#[derive(Debug)]
pub(super) struct NotCompared {
    pub(super) message: String,
    pub(super) unverifiable: bool,
}

impl From<String> for NotCompared {
    fn from(message: String) -> Self {
        Self {
            message,
            unverifiable: false,
        }
    }
}

/// A command as [`known`] keeps it: its words and directory, with paths
/// into the checkout written from its root, so every worktree of the
/// repository shares it.
fn command_key(job: &Job, root: &Path) -> String {
    let dot = Path::new(".");
    let words: Vec<String> = job.argv.iter().map(|w| rebase(w, root, dot)).collect();
    let dir = job.dir.strip_prefix(root).unwrap_or(Path::new(""));
    format!("{} @{}", words.join(" "), dir.display())
}

/// A worktree of the merge base, checked out when a job first needs it.
struct Checkout<'a> {
    root: &'a Path,
    base: &'a affected::Base,
    tmp: Option<(tempfile::TempDir, PathBuf)>,
}

impl Checkout<'_> {
    fn tree(&mut self) -> Result<&Path, String> {
        if self.tmp.is_none() {
            let tmp = tempfile::Builder::new()
                .prefix("jouet-baseline-")
                .tempdir()
                .map_err(|e| format!("making a directory for the merge base: {e}"))?;
            let tree = tmp.path().join("tree");
            let tree_arg = tree.to_string_lossy().into_owned();
            git(
                self.root,
                &[
                    "worktree",
                    "add",
                    "--detach",
                    "--quiet",
                    &tree_arg,
                    &self.base.merge_base,
                ],
            )
            .map_err(|e| format!("checking out the merge base {}: {e}", self.base.merge_base))?;
            // Keep the worktree registered for cleanup even if local setup fails.
            self.tmp = Some((tmp, tree));
            let tree = &self.tmp.as_ref().expect("just checked out").1;
            if tree.join(".gitmodules").is_file() {
                #[cfg(unix)]
                local_submodules(self.root, tree)?;
                // Also initializes anything that had no usable local source.
                let _ = git(tree, &["submodule", "update", "--init", "--quiet"]);
            }
        }
        Ok(&self.tmp.as_ref().expect("checked out above").1)
    }
}

impl Drop for Checkout<'_> {
    fn drop(&mut self) {
        if let Some((_, tree)) = &self.tmp {
            let tree = tree.to_string_lossy().into_owned();
            let _ = git(self.root, &["worktree", "remove", "--force", &tree]);
        }
    }
}

/// Runs only the failing Go tests of `now` at the merge base, and returns
/// their names when every one fails there with the same details: seconds,
/// where the command that found them may take minutes.
fn targeted(
    job: &Job,
    now: &Evidence,
    root: &Path,
    checkout: &mut Checkout<'_>,
    formats: &[Format],
    log: PathBuf,
) -> Result<Option<BTreeSet<String>>, NotCompared> {
    let Some(go) = &now.go else {
        return Ok(None);
    };
    if now.names.iter().any(|name| !now.details.contains_key(name)) {
        return Ok(None);
    }
    // A subtest runs with its test.
    let tests: BTreeSet<&str> = now
        .names
        .iter()
        .map(|name| name.split('/').next().unwrap_or(name))
        .collect();
    let packages: BTreeSet<&str> = go
        .values()
        .flatten()
        .filter_map(|qualified| qualified.rsplit_once("::").map(|(package, _)| package))
        .collect();
    if packages.is_empty() {
        return Ok(None);
    }
    let tree = checkout.tree()?.to_path_buf();
    let dir = job.dir.strip_prefix(root).unwrap_or(Path::new(""));
    let mut argv = vec![
        "go".to_string(),
        "test".into(),
        "-run".into(),
        format!("^({})$", tests.into_iter().collect::<Vec<_>>().join("|")),
    ];
    argv.extend(packages.into_iter().map(str::to_string));
    let at_base = Job {
        argv,
        env: job.env.clone(),
        dir: tree.join(dir),
        exit: Box::new(|_| Exit::Ok),
    };
    let Ok((code, _)) = execute(&at_base, &log, false) else {
        return Ok(None);
    };
    if code == 0 {
        return Ok(None);
    }
    let output = std::fs::read(&log).unwrap_or_default();
    let output = String::from_utf8_lossy(&output);
    let Some(base) = failure_evidence(&output, formats, &tree, false) else {
        return Ok(None);
    };
    // One pattern runs in every package, so a test of the same name may
    // fail in another: each must fail in its own package there.
    let Some(base_go) = &base.go else {
        return Ok(None);
    };
    let same = now.names.iter().all(|name| {
        base.names.contains(name)
            && now.details.get(name) == base.details.get(name)
            && go
                .get(name)
                .zip(base_go.get(name))
                .is_some_and(|(here, there)| here.is_subset(there))
    });
    Ok(same.then(|| now.names.clone()))
}

/// Populate exact pinned commits from already initialized local submodules.
#[cfg(unix)]
fn local_submodules(root: &Path, tree: &Path) -> Result<(), String> {
    use std::path::Component;

    let Some(entries) = git_stdout(
        tree,
        &[
            "config",
            "--null",
            "--file",
            ".gitmodules",
            "--get-regexp",
            r"^submodule\..*\.path$",
        ],
    ) else {
        return Ok(());
    };
    let Ok(root) = root.canonicalize() else {
        return Ok(());
    };
    for entry in entries
        .split(|byte| *byte == 0)
        .filter(|entry| !entry.is_empty())
    {
        let Ok(entry) = std::str::from_utf8(entry) else {
            continue;
        };
        let Some((key, path)) = entry.split_once('\n') else {
            continue;
        };
        let Some(name) = key
            .strip_prefix("submodule.")
            .and_then(|key| key.strip_suffix(".path"))
        else {
            continue;
        };
        let relative = Path::new(path);
        if relative
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
        {
            continue;
        }
        let Ok(source) = root.join(relative).canonicalize() else {
            continue;
        };
        if !source.starts_with(&root) {
            continue;
        }
        let Some(top) = git_stdout(&source, &["rev-parse", "--show-toplevel"])
            .and_then(|output| String::from_utf8(output).ok())
            .and_then(|output| Path::new(output.trim_end()).canonicalize().ok())
        else {
            continue;
        };
        if top != source {
            continue;
        }
        let Some(index) = git_stdout(
            tree,
            &[
                "--literal-pathspecs",
                "ls-files",
                "--stage",
                "-z",
                "--",
                path,
            ],
        )
        .and_then(|output| String::from_utf8(output).ok()) else {
            continue;
        };
        let Some((header, indexed_path)) = index.trim_end_matches('\0').split_once('\t') else {
            continue;
        };
        let mut fields = header.split_whitespace();
        let (Some("160000"), Some(oid), Some("0"), None) =
            (fields.next(), fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        if indexed_path != path
            || !matches!(oid.len(), 40 | 64)
            || !oid.bytes().all(|b| b.is_ascii_hexdigit())
            || git(&source, &["cat-file", "-e", &format!("{oid}^{{commit}}")]).is_err()
        {
            continue;
        }
        let Some(source) = source.to_str() else {
            continue;
        };
        let mut url = String::from("file://");
        for byte in source.bytes() {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'-' | b'_' | b'.' | b'~') {
                url.push(char::from(byte));
            } else {
                use std::fmt::Write;
                let _ = write!(url, "%{byte:02X}");
            }
        }
        // Let Git resolve relative URLs as it normally does. The per-command
        // override never replaces that configured URL in the parent repository.
        if git(tree, &["submodule", "init", "--", path]).is_err() {
            continue;
        }
        let config = format!("submodule.{name}.url");
        let Some(original) = git_stdout(tree, &["config", "--null", "--get", &config])
            .and_then(|output| String::from_utf8(output).ok())
        else {
            continue;
        };
        let original = original.trim_end_matches('\0');
        let override_url = format!("{config}={url}");
        let _ = Command::new("git")
            .arg("-C")
            .arg(tree)
            .args([
                "-c",
                &override_url,
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "update",
                "--init",
                "--quiet",
                "--",
                path,
            ])
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_ALLOW_PROTOCOL", "file")
            .stdin(Stdio::null())
            .output();
        let checkout = tree.join(relative);
        // A failed checkout can still have created a clone. Restore its remote
        // before the normal fallback, too.
        if checkout.join(".git").exists()
            && git_stdout(&checkout, &["remote", "get-url", "origin"]).is_some()
        {
            git(&checkout, &["remote", "set-url", "origin", original])
                .map_err(|_| format!("restoring the original remote for submodule {path}"))?;
        }
    }
    Ok(())
}

#[cfg(unix)]
fn git_stdout(dir: &Path, args: &[&str]) -> Option<Vec<u8>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .output()
        .ok()?;
    output.status.success().then_some(output.stdout)
}

/// Sorts each failed job's failures into those the merge base has too and
/// the new ones, from what [`known`] keeps for the job at `base`, or by
/// running it again in a worktree of `base` and keeping what came of it.
/// [`Mode::Retry`] also retries an inconclusive cached run; [`Mode::Kept`]
/// returns None when a job has nothing kept.
pub(super) fn compare(
    root: &Path,
    base: affected::Base,
    formats: &[Format],
    failed: &[(&Job, Option<Evidence>, u64, u64)],
    mode: Mode,
) -> Result<Option<Baseline>, NotCompared> {
    let setup = affected::project_config::baseline_setup(root).map_err(|e| {
        let hint = e.hint.map_or(String::new(), |hint| format!(" {hint}"));
        NotCompared::from(format!("baseline setup configuration: {}{hint}", e.message))
    })?;
    let mut checkout = Checkout {
        root,
        base: &base,
        tmp: None,
    };
    let mut pre_existing = BTreeSet::new();
    let mut new = BTreeSet::new();
    let mut unknown = BTreeSet::new();
    let mut prepared = false;
    for (i, (job, now, ran, not_failed)) in failed.iter().enumerate() {
        let key = command_key(job, root);
        let kept = if setup.is_empty() {
            known::lookup(root, &base.merge_base, &key).filter(|outcome| {
                !matches!(
                    outcome,
                    known::Outcome::Failed {
                        non_failed: None,
                        ..
                    } | known::Outcome::Failed { details: None, .. }
                )
                    && !matches!(outcome, known::Outcome::Failed { details_version, .. } if *details_version != Some(DETAILS_VERSION))
            })
        } else {
            None
        };
        // Failures that are all Go tests: just those run at the merge base
        // first. One that fails there the same way was there before; any
        // other answer goes on to the whole command, the only run that may
        // call a failure new.
        if kept.is_none()
            && setup.is_empty()
            && mode != Mode::Fresh
            && let Some(now) = now
            && let Some(found) = targeted(
                job,
                now,
                root,
                &mut checkout,
                formats,
                log_path(failed.len() * 2 + 2 + i),
            )?
        {
            pre_existing.extend(found);
            continue;
        }
        let outcome = match (kept, mode) {
            (_, Mode::Fresh)
            | (Some(known::Outcome::Unnamed { .. }), Mode::Retry)
            | (None, Mode::Run | Mode::Retry) => {
                let tree = checkout.tree()?.to_path_buf();
                if !prepared && !setup.is_empty() {
                    let prepare = Job {
                        argv: setup.iter().map(|w| rebase(w, root, &tree)).collect(),
                        env: Vec::new(),
                        dir: tree.clone(),
                        exit: Box::new(|_| Exit::Ok),
                    };
                    let log = log_path(failed.len() * 2 + 1);
                    let (code, _) = execute(&prepare, &log, false)
                        .map_err(|e| format!("baseline setup: {e}"))?;
                    if code != 0 {
                        return Err(NotCompared::from(format!(
                            "baseline setup failed (exit {code}); failures were not compared (full log: {})",
                            log.display()
                        )));
                    }
                    prepared = true;
                }
                let dir = job.dir.strip_prefix(root).unwrap_or(Path::new(""));
                let at_base = Job {
                    argv: job.argv.iter().map(|w| rebase(w, root, &tree)).collect(),
                    env: job.env.clone(),
                    dir: tree.join(dir),
                    exit: Box::new(|_| Exit::Ok),
                };
                // After this run's own logs, so pruning never takes one of them.
                let log = log_path(failed.len() + i + 1);
                let (code, _) = execute(&at_base, &log, false).map_err(|error| {
                    let program = Path::new(&at_base.argv[0]);
                    let program = if program.is_absolute() {
                        program.to_path_buf()
                    } else {
                        at_base.dir.join(program)
                    };
                    if setup.is_empty()
                        && program.starts_with(&tree)
                        && program.components().any(|part| part.as_os_str() == "node_modules")
                        && !program.exists()
                    {
                        format!("{error}; the baseline checkout has no project-local node_modules executable: review [baseline].setup in .jouet/test.toml and trust it before retrying")
                    } else {
                        error
                    }
                })?;
                let output = std::fs::read(&log).unwrap_or_default();
                let output = String::from_utf8_lossy(&output);
                if code == 0
                    && testlog::digest_with(&output, false, 0, formats).failed > 0
                    && super::super::hook::checks(&at_base.dir, &super::command_line(&at_base.argv))
                        .iter()
                        .any(|record| record.kind == jouet_rules::Kind::Test)
                {
                    return Err(NotCompared::from(format!(
                        "the run at {} exited 0 but reported failing tests; failures cannot be compared (full log: {})",
                        base.name,
                        log.display()
                    )));
                }
                let outcome = if code == 0 {
                    known::Outcome::Passed
                } else {
                    match failure_evidence(&output, formats, &tree, mode == Mode::Fresh) {
                        Some(evidence) => {
                            let d = testlog::digest_with(&output, false, 0, formats);
                            known::Outcome::Failed {
                                names: evidence.names.into_iter().collect(),
                                ran: d.passed + d.failed + d.skipped,
                                non_failed: Some(d.passed + d.skipped),
                                details: Some(evidence.details),
                                details_version: Some(DETAILS_VERSION),
                            }
                        }
                        None => known::Outcome::Unnamed {
                            exit: code,
                            log: Some(log.display().to_string()),
                        },
                    }
                };
                if mode != Mode::Fresh && setup.is_empty() {
                    known::keep(root, &base.merge_base, &key, outcome.clone());
                }
                outcome
            }
            (Some(outcome), _) => outcome,
            (None, Mode::Kept) => return Ok(None),
        };
        let (at_base, base_details, reached): (BTreeSet<String>, BTreeMap<String, String>, u64) =
            match outcome {
                known::Outcome::Passed if now.is_none() => {
                    return Err(NotCompared::from(format!(
                        "the command passed at {}, but the current failure could not be named; it remains unresolved",
                        base.name
                    )));
                }
                known::Outcome::Passed => (BTreeSet::new(), BTreeMap::new(), u64::MAX),
                known::Outcome::Failed { .. } if now.is_none() => {
                    return Err(NotCompared {
                        message: format!(
                            "the command also failed at {}, but the current failure could not be named, so the failures cannot be compared; it is not known whether they are the same and the failure remains unresolved",
                            base.name
                        ),
                        unverifiable: true,
                    });
                }
                known::Outcome::Failed {
                    names,
                    ran: base_ran,
                    non_failed,
                    details,
                    details_version: _,
                } => {
                    // A broken checkout can make every test fail in setup.
                    // Matching names in that run do not establish that the
                    // current failures existed. When the current run shows
                    // no test that passed or was skipped either, as when only
                    // failures are printed, the two runs look alike, and the
                    // details compared below tell a setup failure apart.
                    if base_ran > 1 && non_failed == Some(0) && *not_failed > 0 {
                        return Err(NotCompared::from(format!(
                            "all {base_ran} tests failed at {}; failures cannot be compared safely because the baseline may not have reached the tests",
                            base.name
                        )));
                    }
                    (
                        names.into_iter().collect(),
                        details.unwrap_or_default(),
                        base_ran,
                    )
                }
                known::Outcome::Unnamed { exit, log } => {
                    let missing_dependencies = setup.is_empty()
                        && log.as_ref().is_some_and(|path| {
                            std::fs::read_to_string(path).is_ok_and(|output| {
                                output
                                    .contains("Local package.json exists, but node_modules missing")
                            })
                        });
                    let hint = if missing_dependencies {
                        "; the baseline checkout reports missing node_modules: review [baseline].setup in .jouet/test.toml and trust it before retrying"
                    } else {
                        ""
                    };
                    let log = log.map_or(String::new(), |l| format!(" (full log: {l})"));
                    return Err(NotCompared {
                        message: format!(
                            "the run at {} failed (exit {exit}) without naming a failing test, so the failures could not be compared; it is not known whether they are the same{hint}{log}",
                            base.name
                        ),
                        unverifiable: true,
                    });
                }
            };
        // Far fewer tests at the merge base: one that is not among its
        // failures may never have run there.
        let short = reached.saturating_mul(2) < *ran;
        for name in now.iter().flat_map(|e| &e.names) {
            if at_base.contains(name) {
                let current = now.as_ref().and_then(|e| e.details.get(name));
                if current.is_some() && current == base_details.get(name) {
                    pre_existing.insert(name.clone());
                } else {
                    unknown.insert(name.clone());
                }
            } else if short {
                unknown.insert(name.clone());
            } else {
                new.insert(name.clone());
            }
        }
    }
    drop(checkout);
    Ok(Some(Baseline {
        base,
        pre_existing: pre_existing.into_iter().collect(),
        new: new.into_iter().collect(),
        unknown: unknown.into_iter().collect(),
    }))
}

/// `name` with each path into `checkout` written from its root: a worktree
/// under `/tmp` can also be named by its canonical path (`/private/tmp` on
/// macOS).
fn from_root(name: &str, checkout: &Path) -> String {
    let mut name = name.to_string();
    let canonical = checkout.canonicalize().ok();
    // Prefer the canonical spelling: on macOS `/var/...` occurs inside
    // `/private/var/...`, so replacing the shorter prefix first corrupts it.
    for dir in canonical
        .as_deref()
        .into_iter()
        .chain(std::iter::once(checkout))
    {
        let raw = dir.to_string_lossy();
        let slash = raw.replace('\\', "/");
        let slash = slash.trim_end_matches('/');
        for prefix in [
            format!("{raw}/"),
            format!("{raw}\\"),
            format!("{slash}/"),
            format!("{}\\", slash.replace('/', "\\")),
        ] {
            name = name.replace(&prefix, "");
        }
    }
    name
}

/// `word`, a command word, with a path into `root` (alone, or an option's
/// value after `=`) pointed at the same path in `tree`, so the run at the
/// merge base does not read the checkout.
fn rebase(word: &str, root: &Path, tree: &Path) -> String {
    let root_text = root.to_string_lossy();
    let tree_text = tree.to_string_lossy();
    let resolved_root = root.canonicalize().ok();
    let at = |path: &str| -> Option<String> {
        let as_path = Path::new(path);
        if as_path.is_absolute()
            && as_path
                .components()
                .any(|part| part == std::path::Component::ParentDir)
            && let Some(root) = resolved_root.as_deref()
        {
            // Resolve `..` through existing symlinks before deciding whether
            // the path belongs to the checkout.
            let prefix = as_path.ancestors().find(|ancestor| {
                ancestor.components().next_back() == Some(std::path::Component::ParentDir)
            })?;
            let suffix = as_path.strip_prefix(prefix).ok()?;
            let resolved = resolve_existing_ancestors(prefix)?;
            let relative = resolved.strip_prefix(root).ok()?;
            return Some(
                tree.join(relative)
                    .join(suffix)
                    .to_string_lossy()
                    .into_owned(),
            );
        }
        if let Some(rest) = path.strip_prefix(root_text.as_ref())
            && (rest.is_empty() || rest.starts_with(['/', '\\']))
        {
            let relative = Path::new(rest.trim_start_matches(['/', '\\']));
            return inside_checkout(relative).then(|| format!("{tree_text}{rest}"));
        }
        // A checkout can have more than one absolute spelling, such as
        // /var and /private/var on macOS. Resolve an ancestor, not the whole
        // path: the file may be missing at the base or be a symlink itself.
        let path = Path::new(path);
        if !path.is_absolute() {
            return None;
        }
        let root = resolved_root.as_deref()?;
        for ancestor in path.ancestors() {
            if ancestor.canonicalize().ok().as_deref() == Some(root) {
                let rest = path.strip_prefix(ancestor).ok()?;
                return inside_checkout(rest)
                    .then(|| tree.join(rest).to_string_lossy().into_owned());
            }
        }
        None
    };
    if let Some(path) = at(word) {
        return path;
    }
    match word.split_once('=') {
        Some((option, value)) => {
            at(value).map_or_else(|| word.to_string(), |v| format!("{option}={v}"))
        }
        None => word.to_string(),
    }
}

/// Resolve existing ancestors and a missing suffix, without following the
/// final component: the baseline must use its own file or executable shim.
fn resolve_existing_ancestors(path: &Path) -> Option<PathBuf> {
    let leaf = path.file_name();
    let parent = if leaf.is_some() { path.parent()? } else { path };
    let (ancestor, mut resolved) = parent.ancestors().find_map(|ancestor| {
        ancestor
            .canonicalize()
            .ok()
            .map(|resolved| (ancestor, resolved))
    })?;
    for part in parent.strip_prefix(ancestor).ok()?.components() {
        match part {
            std::path::Component::Normal(name) => resolved.push(name),
            std::path::Component::ParentDir if !resolved.pop() => return None,
            std::path::Component::ParentDir | std::path::Component::CurDir => {}
            _ => return None,
        }
    }
    if let Some(leaf) = leaf {
        resolved.push(leaf);
    }
    Some(resolved)
}

/// Reject a relative path that climbs above the checkout before returning.
fn inside_checkout(path: &Path) -> bool {
    let mut depth = 0_usize;
    for part in path.components() {
        match part {
            std::path::Component::Normal(_) => depth += 1,
            std::path::Component::ParentDir => match depth.checked_sub(1) {
                Some(next) => depth = next,
                None => return false,
            },
            std::path::Component::CurDir => {}
            _ => return false,
        }
    }
    true
}

fn git(dir: &Path, args: &[&str]) -> Result<(), String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("running git: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rebases_paths_into_the_checkout() {
        let (root, tree) = (Path::new("/src/app"), Path::new("/tmp/base/tree"));
        for (word, want) in [
            ("/src/app", "/tmp/base/tree"),
            ("/src/app/Cargo.toml", "/tmp/base/tree/Cargo.toml"),
            (
                "--manifest-path=/src/app/Cargo.toml",
                "--manifest-path=/tmp/base/tree/Cargo.toml",
            ),
            ("/src/apple/x", "/src/apple/x"),
            ("/src/app/../other/check", "/src/app/../other/check"),
            ("/src/app/dir/../file", "/tmp/base/tree/dir/../file"),
            ("cargo", "cargo"),
            ("--jobs=4", "--jobs=4"),
        ] {
            assert_eq!(rebase(word, root, tree), want, "{word}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn rebases_an_alias_of_the_checkout_even_when_the_file_is_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("checkout");
        let alias = tmp.path().join("alias");
        let tree = tmp.path().join("baseline");
        std::fs::create_dir(&root).unwrap();
        std::os::unix::fs::symlink(&root, &alias).unwrap();
        let root = root.canonicalize().unwrap();
        let path = alias.join("node_modules/.bin/vitest");
        let want = tree.join("node_modules/.bin/vitest");
        assert_eq!(
            rebase(&path.to_string_lossy(), &root, &tree),
            want.to_string_lossy()
        );
        assert_eq!(
            rebase(&format!("--runner={}", path.display()), &root, &tree),
            format!("--runner={}", want.display())
        );
        let outside = alias.join("../other/check");
        assert_eq!(
            rebase(&outside.to_string_lossy(), &root, &tree),
            outside.to_string_lossy()
        );
        let outside_dir = tmp.path().join("outside/dir");
        std::fs::create_dir_all(&outside_dir).unwrap();
        std::os::unix::fs::symlink(&outside_dir, root.join("external-link")).unwrap();
        let through_link = alias.join("external-link/../missing");
        assert_eq!(
            rebase(&through_link.to_string_lossy(), &root, &tree),
            through_link.to_string_lossy()
        );
        std::fs::create_dir(root.join("subdir")).unwrap();
        std::fs::create_dir_all(root.join("node_modules/.bin")).unwrap();
        let external_runner = outside_dir.join("runner");
        std::fs::write(&external_runner, "").unwrap();
        std::os::unix::fs::symlink(&external_runner, root.join("node_modules/.bin/vitest"))
            .unwrap();
        let shim = alias.join("subdir/../node_modules/.bin/vitest");
        assert_eq!(
            rebase(&shim.to_string_lossy(), &root, &tree),
            tree.join("node_modules/.bin/vitest").to_string_lossy()
        );
        std::os::unix::fs::symlink(&outside_dir, root.join("external-modules")).unwrap();
        let linked_dir = alias.join("subdir/../external-modules/runner");
        assert_eq!(
            rebase(&linked_dir.to_string_lossy(), &root, &tree),
            tree.join("external-modules/runner").to_string_lossy()
        );
    }

    #[test]
    fn a_command_is_kept_the_same_from_every_worktree() {
        let job = |root: &str| Job {
            argv: vec![
                "cargo".into(),
                "test".into(),
                format!("--manifest-path={root}/crates/a/Cargo.toml"),
            ],
            env: Vec::new(),
            dir: Path::new(root).join("crates/a"),
            exit: Box::new(|_| Exit::Ok),
        };
        let here = command_key(&job("/src/app"), Path::new("/src/app"));
        assert_eq!(
            here,
            "cargo test --manifest-path=./crates/a/Cargo.toml @crates/a"
        );
        assert_eq!(command_key(&job("/wt/app-2"), Path::new("/wt/app-2")), here);
    }

    #[test]
    fn an_unrecognised_failure_names_nothing() {
        assert_eq!(
            failure_names("something broke\n", &[], Path::new("/x"), false),
            None
        );
    }

    #[test]
    fn remark_frail_requires_complete_matching_diagnostics() {
        let output = "$ git ls-files '*.md' | xargs -0 remark --use remark-validate-links --frail --quiet\n\
docs/README.md\n\
5:149-5:212 warning Cannot find heading `#l13` missing-heading-in-file remark-validate-links:missing-heading-in-file\n\
6:162-6:230 warning Cannot find heading `#l97` missing-heading-in-file remark-validate-links:missing-heading-in-file\n\
\n⚠ 2 warnings\n[ELIFECYCLE] Command failed with exit code 1.\n";
        let evidence = |text: &str| failure_evidence(text, &[], Path::new("/work"), false);
        let original = evidence(output).unwrap();
        assert_eq!(original.names.len(), 2);
        assert_eq!(original.names, evidence(output).unwrap().names);
        assert_eq!(original.details, evidence(output).unwrap().details);
        let changed = evidence(&output.replace("`#l13`", "`#different` ")).unwrap();
        assert_eq!(original.names, changed.names);
        assert_ne!(original.details, changed.details);
        assert!(evidence(&output.replace("--frail", "--no-frail")).is_none());
        assert!(evidence(&output.replace("xargs -0 remark", "echo remark")).is_none());
        assert!(evidence(&output.replace("⚠ 2 warnings", "⚠ 3 warnings")).is_none());
        assert!(evidence(&output.replace("⚠ 2 warnings", "other.md\n⚠ 2 warnings")).is_none());
        assert!(
            evidence(&output.replace("[ELIFECYCLE] Command failed with exit code 1.", ""))
                .is_none()
        );
        assert!(evidence(&format!("{output}another error\n")).is_none());
        assert!(evidence(&format!("Earlier task failed unexpectedly\n{output}")).is_none());
        assert!(evidence(&format!("ERROR:/src/other.rs\n{output}")).is_none());
        assert!(evidence(&format!("not ok 1 unnamed\n{output}")).is_none());
        assert!(evidence(&format!("Earlier task: 0 errors\n{output}")).is_some());
        assert!(evidence(&format!("3 passed, 0 failed, 0 failures\n{output}")).is_some());
        assert!(evidence(&format!("0 failed, 1 error\n{output}")).is_none());
        assert!(evidence(&format!("Checked docs/error-handling.md\n{output}")).is_some());
        assert!(evidence(&output.replace("docs/README.md", "docs/Release Notes.md")).is_some());
        assert!(evidence(&output.replace("[ELIFECYCLE]", "ELIFECYCLE ")).is_some());
        assert!(failure_evidence(output, &[], Path::new("/work"), true).is_none());
    }

    #[test]
    fn failure_evidence_compares_full_details_and_normalizes_checkout_paths() {
        let evidence = |root: &str, detail: &str| {
            let output = format!(
                "test parse_input ... FAILED\n---- parse_input stdout ----\nthread 'parse_input' (1234) panicked at {root}/src/a.rs:2:\n{detail}\n\ntest result: FAILED. 1 passed; 1 failed; 0 ignored\n"
            );
            failure_evidence(&output, &[], Path::new(root), false).unwrap()
        };
        let here = evidence("/src/app", "assertion left: 2 right: 3");
        let same = evidence("/tmp/base/tree", "assertion left: 2 right: 3");
        let different = evidence("/tmp/base/tree", "ModuleNotFoundError: dependency");
        assert_eq!(here.details, same.details);
        assert!(!here.details.is_empty());
        assert_ne!(here.details, different.details);

        // Differences beyond the displayed excerpt still change the fingerprint.
        let long = |last: &str| format!("{}\n{last}", "line\n".repeat(100));
        assert_ne!(
            evidence("/src/app", &long("first")).details,
            evidence("/src/app", &long("second")).details
        );
    }

    #[test]
    fn name_only_and_ambiguous_failures_have_no_details() {
        let names_only =
            "test parse_input ... FAILED\ntest result: FAILED. 1 passed; 1 failed; 0 ignored\n";
        let evidence = failure_evidence(names_only, &[], Path::new("/x"), false).unwrap();
        assert!(!evidence.names.is_empty());
        assert!(evidence.details.is_empty());

        let pytest_summary = "FAILED tests/test_a.py::test_x - AssertionError: failure\n";
        let evidence = failure_evidence(pytest_summary, &[], Path::new("/x"), false).unwrap();
        assert!(!evidence.names.is_empty());
        assert!(evidence.details.is_empty());

        let repeated = "test same ... FAILED\n---- same stdout ----\nfirst cause\n---- same stdout ----\nsecond cause\ntest result: FAILED. 1 passed; 1 failed; 0 ignored\n";
        let evidence = failure_evidence(repeated, &[], Path::new("/x"), false).unwrap();
        assert!(evidence.details.is_empty());
    }

    #[test]
    fn playwright_baselines_require_matching_failure_details() {
        let output = |received: &str| {
            format!(
                "Running 1 test using 1 worker\n  1) e2e/balance.spec.ts:5:3 › updates balance\n    Error: expect(locator).toHaveText(expected) failed\n    Expected: 42\n    Received: {received}\n  1 failed\n"
            )
        };
        let evidence =
            |received| failure_evidence(&output(received), &[], Path::new("/work"), false).unwrap();
        assert!(!evidence("17").details.is_empty());
        assert_eq!(evidence("17").details, evidence("17").details);
        assert_ne!(evidence("17").details, evidence("18").details);

        for prefix in ["", "Error: ", "TimeoutError: "] {
            let timeout = format!(
                "Running 1 test using 1 worker, shard 1 of 2\n  1) e2e/balance.spec.ts:5:3 › updates balance\n    {prefix}Test timeout of 30000ms exceeded.\n  1 failed\n"
            );
            let evidence = failure_evidence(&timeout, &[], Path::new("/work"), false).unwrap();
            assert!(!evidence.names.is_empty());
            assert!(evidence.details.is_empty(), "{prefix}");
        }
    }

    #[test]
    fn pytest_class_headers_match_qualified_node_ids() {
        let output = include_str!("../../testlog/fixtures/pytest-q.txt");
        let evidence = failure_evidence(output, &[], Path::new("/work"), false).unwrap();
        assert!(
            evidence
                .details
                .contains_key("tests/test_calc.py::TestStrings::test_upper")
        );

        let output = "=== FAILURES ===\n___ test_same ___\nE first cause\ntests/a.py:5: AssertionError\n___ test_same ___\nE second cause\ntests/b.py:7: AssertionError\n=== short test summary info ===\nFAILED tests/a.py::test_same - AssertionError\nFAILED tests/b.py::test_same - AssertionError\n2 failed in 0.1s\n";
        let evidence = failure_evidence(output, &[], Path::new("/work"), false).unwrap();
        assert!(evidence.details.contains_key("tests/a.py::test_same"));
        assert!(evidence.details.contains_key("tests/b.py::test_same"));
        assert_ne!(
            evidence.details["tests/a.py::test_same"],
            evidence.details["tests/b.py::test_same"]
        );

        let missing = "=== FAILURES ===\n___ test_same ___\nE first cause\ntests/a.py:5: AssertionError\n=== short test summary info ===\nFAILED tests/a.py::test_same - AssertionError\nFAILED tests/b.py::test_same - AssertionError\n2 failed in 0.1s\n";
        let evidence = failure_evidence(missing, &[], Path::new("/work"), false).unwrap();
        assert!(!evidence.details.contains_key("tests/b.py::test_same"));
    }

    #[test]
    fn rust_panic_thread_ids_are_normalized_when_indented() {
        let line = |id| format!("    thread 'test' ({id}) panicked at src/lib.rs:1:");
        assert_eq!(
            RUST_THREAD_ID.replace(&line(123), "$1$2"),
            RUST_THREAD_ID.replace(&line(456), "$1$2")
        );
    }

    #[test]
    fn go_panic_stacks_compare_without_goroutines_or_addresses() {
        let output = |goroutine: u32, address: &str, message: &str| {
            format!(
                "--- FAIL: TestX (0.00s)\npanic: {message} [recovered]\n\tpanic: {message}\n\ngoroutine {goroutine} [running]:\ntesting.tRunner.func1.2({{{address}, 0x6c8b10}})\n\t/usr/lib/go/src/testing/testing.go:1734 +0x{goroutine}c\nexample.com/p.TestX({address}?)\n\t/w/p/p_test.go:9 +0x25\ncreated by testing.(*T).Run in goroutine 1\n\t/usr/lib/go/src/testing/testing.go:1851 +0x413\nFAIL\texample.com/p\t0.004s\nFAIL\n"
            )
        };
        let details = |text: String| {
            let evidence = failure_evidence(&text, &[], Path::new("/w"), false).unwrap();
            // The test that panicked has the panic as its details.
            assert!(evidence.details.contains_key("TestX"), "{text}");
            evidence.details
        };
        assert_eq!(
            details(output(7, "0x5e0f40", "boom")),
            details(output(4021, "0xc000102380", "boom"))
        );
        assert_ne!(
            details(output(7, "0x5e0f40", "boom")),
            details(output(7, "0x5e0f40", "bang"))
        );
    }

    #[test]
    fn pytest_self_addresses_are_volatile_but_assertions_are_not() {
        let output = |address: &str, assertion: &str| {
            format!(
                "=== FAILURES ===\n___ TestParser.test_parse ___\nself = <test_parser.TestParser object at {address}>\nE {assertion}\ntests/test_parser.py:7: AssertionError\n=== short test summary info ===\nFAILED tests/test_parser.py::TestParser::test_parse - AssertionError\n1 failed in 0.1s\n"
            )
        };
        let evidence = |address, assertion| {
            failure_evidence(&output(address, assertion), &[], Path::new("/work"), false).unwrap()
        };
        assert_eq!(
            evidence("0x1234", "assert 1 == 2").details,
            evidence("0xabcd", "assert 1 == 2").details
        );
        assert!(!evidence("0x1234", "assert 1 == 2").details.is_empty());
        assert_ne!(
            evidence("0x1234", "assert 1 == 2").details,
            evidence("0xabcd", "assert 1 == 3").details
        );
        assert_ne!(
            evidence("0x1234", "assert 0x1234 == 2").details,
            evidence("0xabcd", "assert 0xabcd == 2").details
        );
    }

    #[test]
    fn checkout_paths_normalize_windows_and_forward_slashes() {
        let root = Path::new(r"C:\work\project");
        assert_eq!(
            from_root(r"at C:\work\project\src\file.rs:3", root),
            r"at src\file.rs:3"
        );
        assert_eq!(
            from_root("at C:/work/project/src/file.rs:3", root),
            "at src/file.rs:3"
        );
        assert_eq!(
            from_root(r"at C:\work\project-other\file.rs:3", root),
            r"at C:\work\project-other\file.rs:3"
        );
        assert_eq!(
            from_root(r"at /tmp/a\b/file.rs:3", Path::new(r"/tmp/a\b")),
            "at file.rs:3"
        );
    }

    #[test]
    fn names_are_written_from_the_checkout_root() {
        let names = |out: &str, root: &str| failure_names(out, &[], Path::new(root), false);
        let here = names("Diff in /src/app/src/a.rs:3:\n-x\n+y\n", "/src/app");
        let there = names("Diff in /tmp/b/tree/src/a.rs:3:\n-x\n+y\n", "/tmp/b/tree");
        assert_eq!(here, there);
        assert_eq!(
            here.unwrap().into_iter().next().unwrap(),
            "rustfmt src/a.rs:3"
        );
    }

    #[test]
    fn affected_go_failures_are_qualified_by_package() {
        let names = |out: &str| failure_names(out, &[], Path::new("/x"), true);
        let first =
            names("--- FAIL: TestSame (0.00s)\nFAIL\nFAIL\texample.com/a\t0.001s\nFAIL\n").unwrap();
        let second =
            names("--- FAIL: TestSame (0.00s)\nFAIL\nFAIL\texample.com/b\t0.001s\nFAIL\n").unwrap();
        assert_ne!(first, second);
        assert!(names("--- FAIL: TestSame (0.00s)\nFAIL\n").is_none());
        let mixed = names("--- FAIL: TestSame (0.00s)\n    --- FAIL: TestSame/sub (0.00s)\nFAIL\nFAIL\texample.com/a\t0.001s\n--- FAIL: TestSame (0.00s)\nFAIL\nFAIL\texample.com/b\t0.001s\nFAIL\n").unwrap();
        assert!(mixed.contains("example.com/a::TestSame/sub"));
        assert!(mixed.contains("example.com/b::TestSame"));

        let details = "--- FAIL: TestSame (0.00s)\n    a_test.go:1: first cause\nFAIL\nFAIL\texample.com/a\t0.001s\n--- FAIL: TestSame (0.00s)\n    b_test.go:2: second cause\nFAIL\nFAIL\texample.com/b\t0.001s\n";
        let evidence = failure_evidence(details, &[], Path::new("/x"), true).unwrap();
        assert!(evidence.details.contains_key("example.com/a::TestSame"));
        assert!(evidence.details.contains_key("example.com/b::TestSame"));
        assert_ne!(
            evidence.details["example.com/a::TestSame"],
            evidence.details["example.com/b::TestSame"]
        );
    }

    #[test]
    fn affected_jest_failures_are_qualified_by_file() {
        let names = |out: &str| failure_names(out, &[], Path::new("/x"), true);
        let first = names("FAIL tests/a.test.js\n  ● same test\n").unwrap();
        let second = names("FAIL tests/b.test.js\n  ● same test\n").unwrap();
        assert_ne!(first, second);
        assert!(names("  ● same test\n").is_none());
        assert_eq!(
            names("FAIL tests/a.test.js (5.123 s)\n  ● same test\n"),
            names("FAIL tests/a.test.js (12.345 s)\n  ● same test\n")
        );
        let details = "FAIL tests/a.test.js\n  ● same test\n    first cause\nFAIL tests/b.test.js\n  ● same test\n    second cause\n";
        let evidence = failure_evidence(details, &[], Path::new("/x"), true).unwrap();
        assert!(evidence.details.contains_key("tests/a.test.js::same test"));
        assert!(evidence.details.contains_key("tests/b.test.js::same test"));
        assert_ne!(
            evidence.details["tests/a.test.js::same test"],
            evidence.details["tests/b.test.js::same test"]
        );
    }

    #[test]
    fn affected_libtest_names_from_multiple_binaries_are_ambiguous() {
        let output = "test same ... FAILED\ntest result: FAILED. 0 passed; 1 failed; 0 ignored\ntest result: ok. 1 passed; 0 failed; 0 ignored\n";
        let evidence = failure_evidence(output, &[], Path::new("/x"), true).unwrap();
        assert!(evidence.details.is_empty());
    }

    #[test]
    fn workspace_libtest_evidence_includes_the_binary_identity() {
        let output = |binary: &str, hash: &str| {
            format!(
                "Running unittests src/lib.rs (target/debug/deps/{binary}-{hash})\n\
             test same ... FAILED\n---- same stdout ----\nassertion failed\n\
             test result: FAILED. 1 passed; 1 failed; 0 ignored\n\
             Running tests/other.rs (target/debug/deps/other-12345678)\n\
             test result: ok. 1 passed; 0 failed; 0 ignored\n"
            )
        };
        let evidence = |text: String| failure_evidence(&text, &[], Path::new("/x"), true).unwrap();
        let a = evidence(output("first", "12345678"));
        assert!(!a.details.is_empty());
        assert_eq!(a.details, evidence(output("first", "abcdef12")).details);
        assert_ne!(a.details, evidence(output("second", "abcdef12")).details);

        let integration = "Running tests/other.rs (target/debug/deps/other-12345678)\ntest same ... FAILED\n---- same stdout ----\nassertion failed\ntest result: FAILED. 1 passed; 1 failed; 0 ignored\nRunning unittests src/lib.rs (target/debug/deps/lib-12345678)\ntest result: ok. 1 passed; 0 failed; 0 ignored\n";
        let integration = evidence(integration.to_string());
        assert!(!integration.details.is_empty());
        assert_ne!(a.details, integration.details);

        let duplicate = format!(
            "{}{}",
            output("first", "12345678"),
            output("second", "abcdef12")
        );
        assert!(evidence(duplicate).details.is_empty());
    }
}

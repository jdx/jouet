//! Rule packs: named sets of rules from a git repository at a pinned
//! commit, or from a directory.
//!
//! A pin in a `packs.toml` names where a pack comes from and the hash of
//! its rules. Fetching resolves a git ref to a full commit, checks it with
//! `git rev-parse`, and copies the rule files of that commit's tree into
//! `<state dir>/packs/<hash>`, where only the user writes. Loading checks
//! the copy against the pin's hash, so a pack never runs rules other than
//! the ones pinned, and a hook never reaches the network.

use jouet_rules::Digests;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

/// A `packs.toml`: `[pack.NAME]` tables.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Pins {
    #[serde(default)]
    pub pack: BTreeMap<String, Pin>,
}

/// Where one pack comes from, and what it must hash to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Pin {
    /// A git URL: https, ssh, or a local path.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub git: Option<String>,
    /// The full commit the pack is pinned at, with `git`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rev: Option<String>,
    /// A local directory, without `git`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// The directory of rule files in the source, such as `rules`.
    pub dir: String,
    /// The SHA-256 of the rule files, as trust hashes a directory.
    pub digest: String,
}

impl Pin {
    /// Where the source is, for a person.
    pub(crate) fn source(&self) -> String {
        match (&self.git, &self.rev, &self.path) {
            (Some(git), Some(rev), _) => format!("{git} at {}", short(rev)),
            (Some(git), None, _) => git.clone(),
            (None, _, Some(path)) => path.clone(),
            (None, _, None) => String::new(),
        }
    }

    /// What is wrong with the pin itself, before anything is read for it.
    pub(crate) fn problem(&self) -> Option<String> {
        if !is_hex(&self.digest, &[64]) {
            return Some("its `digest` is not a SHA-256 in hex".into());
        }
        if is_unsafe_subdir(&self.dir) {
            return Some(format!(
                "its `dir` must be a path inside the pack, not `{}`",
                self.dir
            ));
        }
        match (&self.git, &self.rev, &self.path) {
            (Some(_), Some(rev), None) if is_hex(rev, &[40, 64]) => None,
            (Some(_), _, None) => Some("a git pack is pinned by `rev`, a full commit hash".into()),
            (None, None, Some(_)) => None,
            _ => Some("a pack names either `git` and `rev`, or `path`".into()),
        }
    }
}

fn short(rev: &str) -> &str {
    rev.get(..12).unwrap_or(rev)
}

fn is_hex(s: &str, lens: &[usize]) -> bool {
    lens.contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn is_unsafe_subdir(dir: &str) -> bool {
    let path = Path::new(dir);
    path.is_absolute()
        || path.components().any(|c| {
            !matches!(
                c,
                std::path::Component::Normal(_) | std::path::Component::CurDir
            )
        })
}

/// Reads a `packs.toml`; a missing one pins nothing.
pub(crate) fn read(path: &Path) -> Result<Pins, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Pins::default()),
        Err(e) => Err(format!("reading {}: {e}", path.display())),
    }
}

pub(crate) fn write(path: &Path, pins: &Pins) -> Result<(), String> {
    let text = toml::to_string(pins).map_err(|e| e.to_string())?;
    let text = format!(
        "# Rule packs, pinned by `jouet rule pack add`; see https://jouet.jdx.dev/rules#rule-packs\n{text}"
    );
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
    }
    std::fs::write(path, text).map_err(|e| format!("writing {}: {e}", path.display()))
}

/// `<state dir>/packs`, where fetched packs are kept by their hash.
pub(crate) fn store() -> Option<PathBuf> {
    jouet_output::paths::state_dir().map(|d| d.join("packs"))
}

/// Where a pin's rules are kept.
pub(crate) fn stored(pin: &Pin) -> Option<PathBuf> {
    store().map(|s| s.join(&pin.digest))
}

/// What a fetch found: the commit, for git, and the pin to record.
pub(crate) struct Fetched {
    pub pin: Pin,
    pub dir: PathBuf,
}

/// Whether a source names a git repository rather than a directory.
pub(crate) fn is_git_url(source: &str) -> bool {
    source.contains("://")
        || (source.contains('@') && source.contains(':') && !Path::new(source).exists())
        || source.ends_with(".git")
}

/// Fetches a pack: a git repository at `rev` (its default branch when
/// none), or a directory, keeping its rule files in the store. With `want`,
/// the fetched rules must hash to it.
pub(crate) fn fetch(
    source: &str,
    git: bool,
    rev: Option<&str>,
    dir: Option<&str>,
    want: Option<&str>,
) -> Result<Fetched, String> {
    let store = store()
        .ok_or("there is no home directory to keep rule packs in; set HOME or XDG_STATE_HOME")?;
    std::fs::create_dir_all(&store).map_err(|e| format!("creating {}: {e}", store.display()))?;
    let staging = tempfile::Builder::new()
        .prefix(".fetch-")
        .tempdir_in(&store)
        .map_err(|e| format!("creating a directory in {}: {e}", store.display()))?;
    let files = staging.path().join("rules");
    let (commit, dir) = if git {
        let (commit, dir) = fetch_git(source, rev, dir, staging.path(), &files)?;
        (Some(commit), dir)
    } else {
        (None, copy_dir(Path::new(source), dir, &files)?)
    };
    let digest = Digests::load(None)
        .of(&files, None)
        .map_err(|e| format!("hashing the pack's rules: {e}"))?;
    if let Some(want) = want
        && want != digest
    {
        return Err(format!(
            "the rules fetched from {source} hash to {digest}, not the pinned {want}"
        ));
    }
    let kept = store.join(&digest);
    if kept.is_dir() {
        // Kept before: the same rules, unless something changed them since.
        if Digests::load(None).of(&kept, None).ok().as_deref() != Some(digest.as_str()) {
            std::fs::remove_dir_all(&kept)
                .map_err(|e| format!("replacing {}: {e}", kept.display()))?;
        }
    }
    if !kept.is_dir() {
        std::fs::rename(&files, &kept)
            .map_err(|e| format!("keeping the pack in {}: {e}", kept.display()))?;
    }
    let pin = Pin {
        git: git.then(|| source.to_string()),
        rev: commit,
        path: (!git).then(|| {
            std::path::absolute(source)
                .map_or_else(|_| source.to_string(), |p| p.display().to_string())
        }),
        dir,
        digest,
    };
    Ok(Fetched { pin, dir: kept })
}

/// Fetches a pinned pack whose rules are missing from the store or do not
/// match its pin.
pub(crate) fn sync(pin: &Pin, digests: &mut Digests) -> Result<bool, String> {
    if let Some(problem) = pin.problem() {
        return Err(problem);
    }
    if let Some(kept) = stored(pin)
        && digests.of(&kept, None).ok().as_deref() == Some(pin.digest.as_str())
    {
        return Ok(false);
    }
    let (source, git) = match (&pin.git, &pin.path) {
        (Some(git), _) => (git.as_str(), true),
        (None, Some(path)) => (path.as_str(), false),
        (None, None) => return Err("the pin names no source".into()),
    };
    fetch(
        source,
        git,
        pin.rev.as_deref(),
        Some(&pin.dir),
        Some(&pin.digest),
    )?;
    Ok(true)
}

/// The rule directories a pack may keep its rules in, in order.
const DIRS: [&str; 2] = ["rules", ".jouet/rules"];

/// Copies the `.toml` and `.luau` files of a directory's rules into `to`,
/// and says which directory they were in.
fn copy_dir(source: &Path, dir: Option<&str>, to: &Path) -> Result<String, String> {
    let dir = match dir {
        Some(dir) => dir.to_string(),
        None => DIRS
            .iter()
            .find(|d| source.join(d).is_dir())
            .map(ToString::to_string)
            .ok_or_else(|| {
                format!(
                    "{} has no rules/ or .jouet/rules/ directory",
                    source.display()
                )
            })?,
    };
    if is_unsafe_subdir(&dir) {
        return Err(format!("--dir must be a path inside the pack, not `{dir}`"));
    }
    let from = source.join(&dir);
    let mut copied = 0;
    copy_rules(&from, &from, to, &mut copied)?;
    if copied == 0 {
        return Err(format!("{} holds no rule files", from.display()));
    }
    Ok(dir)
}

fn copy_rules(root: &Path, dir: &Path, to: &Path, copied: &mut usize) -> Result<(), String> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| format!("reading {}: {e}", dir.display()))?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .collect();
    entries.sort();
    for path in entries {
        let meta = std::fs::symlink_metadata(&path)
            .map_err(|e| format!("reading {}: {e}", path.display()))?;
        if meta.is_dir() {
            copy_rules(root, &path, to, copied)?;
        } else if meta.is_file() && is_rule_file(&path) {
            let rel = path.strip_prefix(root).unwrap_or(&path);
            let target = to.join(rel);
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            std::fs::copy(&path, &target)
                .map_err(|e| format!("copying {}: {e}", path.display()))?;
            *copied += 1;
        }
    }
    Ok(())
}

fn is_rule_file(path: &Path) -> bool {
    path.extension().is_some_and(|e| e == "toml" || e == "luau")
}

/// git with no prompts, no hooks, and only transports that do not run a
/// command named in the URL.
fn git(git_dir: &Path) -> Command {
    let mut c = Command::new("git");
    c.arg("--git-dir")
        .arg(git_dir)
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "protocol.allow=never",
        ])
        .args([
            "-c",
            "protocol.https.allow=always",
            "-c",
            "protocol.http.allow=always",
        ])
        .args([
            "-c",
            "protocol.ssh.allow=always",
            "-c",
            "protocol.file.allow=always",
        ])
        .env("GIT_TERMINAL_PROMPT", "0")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .stdin(std::process::Stdio::null());
    c
}

fn run(mut c: Command, what: &str) -> Result<Vec<u8>, String> {
    let out = c.output().map_err(|e| format!("running git: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "{what}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(out.stdout)
}

/// Fetches `rev` of the repository at `url` into a bare repository in
/// `scratch`, checks the commit, and writes the rule files of its tree to
/// `to`. Returns the full commit and the directory the rules were in.
fn fetch_git(
    url: &str,
    rev: Option<&str>,
    dir: Option<&str>,
    scratch: &Path,
    to: &Path,
) -> Result<(String, String), String> {
    if url.starts_with('-') {
        return Err(format!("`{url}` is not a git URL"));
    }
    let repo = scratch.join("git");
    let mut init = Command::new("git");
    init.args(["init", "--quiet", "--bare"]).arg(&repo);
    run(init, "creating a repository to fetch into")?;
    let want = rev.unwrap_or("HEAD");
    let pinned = rev.filter(|r| is_hex(r, &[40, 64]));
    let mut shallow = git(&repo);
    shallow.args([
        "fetch",
        "--quiet",
        "--no-tags",
        "--depth",
        "1",
        "--",
        url,
        want,
    ]);
    let commit_of = match run(shallow, "fetching") {
        Ok(_) => "FETCH_HEAD".to_string(),
        // A server that will not send a commit by its hash sends its
        // branches and tags, among which the commit must be.
        Err(_) => {
            let mut all = git(&repo);
            all.args(["fetch", "--quiet", "--", url])
                .args(["+refs/heads/*:refs/heads/*", "+refs/tags/*:refs/tags/*"]);
            run(all, &format!("fetching {url}"))?;
            want.to_string()
        }
    };
    let mut parse = git(&repo);
    parse
        .args(["rev-parse", "--verify", "--end-of-options"])
        .arg(format!("{commit_of}^{{commit}}"));
    let commit = String::from_utf8_lossy(&run(parse, &format!("finding {want} in {url}"))?)
        .trim()
        .to_string();
    if let Some(pinned) = pinned
        && commit != pinned
    {
        return Err(format!(
            "{url} gave commit {commit} for the pinned {pinned}"
        ));
    }
    let dir = match dir {
        Some(dir) => dir.to_string(),
        None => DIRS
            .iter()
            .find(|d| {
                let mut kind = git(&repo);
                kind.args(["cat-file", "-t", &format!("{commit}:{d}")]);
                run(kind, "").is_ok_and(|out| out.starts_with(b"tree"))
            })
            .map(ToString::to_string)
            .ok_or_else(|| {
                format!(
                    "{url} has no rules/ or .jouet/rules/ directory at {}",
                    short(&commit)
                )
            })?,
    };
    if is_unsafe_subdir(&dir) {
        return Err(format!("--dir must be a path inside the pack, not `{dir}`"));
    }
    let mut tree = git(&repo);
    tree.args(["ls-tree", "-r", "-z", "--full-tree", &commit, "--"])
        .arg(&dir);
    let listing = run(tree, &format!("listing {dir} at {}", short(&commit)))?;
    let prefix = format!("{}/", dir.trim_end_matches('/'));
    let mut copied = 0;
    for entry in listing.split(|b| *b == 0).filter(|e| !e.is_empty()) {
        let entry = String::from_utf8_lossy(entry);
        let Some((meta, path)) = entry.split_once('\t') else {
            continue;
        };
        let mut meta = meta.split(' ');
        let (Some(mode), Some(kind), Some(oid)) = (meta.next(), meta.next(), meta.next()) else {
            continue;
        };
        // Only plain files: never a symlink out of the pack.
        let Some(rel) = path.strip_prefix(&prefix) else {
            continue;
        };
        if kind != "blob" || !matches!(mode, "100644" | "100755") || !is_rule_file(Path::new(rel)) {
            continue;
        }
        if is_unsafe_subdir(rel) {
            continue;
        }
        let mut blob = git(&repo);
        blob.args(["cat-file", "blob", oid]);
        let bytes = run(blob, &format!("reading {path}"))?;
        let target = to.join(rel);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::write(&target, bytes).map_err(|e| format!("writing {}: {e}", target.display()))?;
        copied += 1;
    }
    if copied == 0 {
        return Err(format!("{dir} at {} holds no rule files", short(&commit)));
    }
    Ok((commit, dir))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pin() -> Pin {
        Pin {
            git: Some("https://example.com/r.git".into()),
            rev: Some("a".repeat(40)),
            path: None,
            dir: "rules".into(),
            digest: "b".repeat(64),
        }
    }

    #[test]
    fn a_pin_names_a_full_commit_and_a_hash() {
        assert_eq!(pin().problem(), None);
        let short = Pin {
            rev: Some("abc123".into()),
            ..pin()
        };
        assert!(short.problem().unwrap().contains("full commit"));
        let branch = Pin {
            rev: Some("main".into()),
            ..pin()
        };
        assert!(branch.problem().is_some());
        let escapes = Pin {
            digest: "../../x".into(),
            ..pin()
        };
        assert!(escapes.problem().is_some());
        let outside = Pin {
            dir: "../x".into(),
            ..pin()
        };
        assert!(outside.problem().is_some());
        let path = Pin {
            git: None,
            rev: None,
            path: Some("/x".into()),
            ..pin()
        };
        assert_eq!(path.problem(), None);
    }

    #[test]
    fn pins_round_trip_through_toml() {
        let pins = Pins {
            pack: BTreeMap::from([("team".to_string(), pin())]),
        };
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("packs.toml");
        write(&path, &pins).unwrap();
        assert_eq!(read(&path).unwrap().pack, pins.pack);
        assert!(read(&dir.path().join("none.toml")).unwrap().pack.is_empty());
    }

    #[test]
    fn git_urls_are_told_from_paths() {
        assert!(is_git_url("https://github.com/acme/rules"));
        assert!(is_git_url("git@github.com:acme/rules.git"));
        assert!(!is_git_url("../team-rules"));
    }
}

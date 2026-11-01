//! Plugins: named sets of rules and routines from a git repository at a
//! pinned commit, or from a directory.
//!
//! A plugin is a `jouet-plugin.toml` manifest with a `rules/` directory, a
//! `routines/` one, or both. A pin in a `plugins.toml` names where a plugin
//! comes from and the hash of every file it ships. Fetching resolves a git
//! ref to a full commit, checks it with `git rev-parse`, and copies the
//! plain files of that commit's tree into `<state dir>/plugins/<hash>`,
//! where only the user writes. Loading checks the copy against the pin's
//! hash, so a plugin never runs anything but what was pinned, and a hook
//! never reaches the network.

use jouet_rules::Digests;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

/// A plugin's manifest, at its root.
pub(crate) const MANIFEST: &str = "jouet-plugin.toml";

/// The kinds of thing a plugin ships, each in a directory of that name.
const KINDS: &[&str] = &["rules", "routines", "formats"];

/// The files a plugin's hash covers: its manifest and everything it ships.
pub(crate) const ENTRIES: &[&str] = &[MANIFEST, "rules", "routines", "formats"];

/// `jouet-plugin.toml`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Manifest {
    /// Lowercase words joined by dashes: its rules and routines are named
    /// `NAME/...`.
    pub name: String,
    /// What it is for, in one line.
    #[serde(default)]
    pub description: String,
    /// The oldest jouet it works with, such as `0.3`.
    #[serde(default)]
    pub jouet: Option<String>,
}

impl Manifest {
    /// Reads and checks the manifest at a plugin's root.
    pub(crate) fn read(root: &Path) -> Result<Self, String> {
        let path = root.join(MANIFEST);
        let text = std::fs::read_to_string(&path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                format!("it has no {MANIFEST}")
            } else {
                format!("reading {MANIFEST}: {e}")
            }
        })?;
        let manifest: Self = toml::from_str(&text).map_err(|e| format!("{MANIFEST}: {e}"))?;
        if !jouet_rules::valid_name(&manifest.name) {
            return Err(format!(
                "{MANIFEST}: `name` must be lowercase words joined by dashes, not `{}`",
                manifest.name
            ));
        }
        if let Some(version) = &manifest.jouet
            && parse_version(version).is_none()
        {
            return Err(format!(
                "{MANIFEST}: `jouet` is the oldest jouet it works with, such as \"0.3\", not `{version}`"
            ));
        }
        Ok(manifest)
    }

    /// The jouet version it needs, when this one is older.
    pub(crate) fn needs(&self) -> Option<&str> {
        let need = self.jouet.as_deref()?;
        let have = parse_version(env!("CARGO_PKG_VERSION"))?;
        (parse_version(need)? > have).then_some(need)
    }
}

/// `1.2.3` as numbers, padded to three; `None` if it is not a version.
fn parse_version(version: &str) -> Option<[u64; 3]> {
    let mut out = [0; 3];
    let parts: Vec<&str> = version.split('.').collect();
    if parts.is_empty() || parts.len() > 3 {
        return None;
    }
    for (slot, part) in out.iter_mut().zip(parts) {
        *slot = part.parse().ok()?;
    }
    Some(out)
}

/// A `plugins.toml`: `[plugin.NAME]` tables.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Pins {
    #[serde(default)]
    pub plugin: BTreeMap<String, Pin>,
}

/// Where one plugin comes from, and what it must hash to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Pin {
    /// A git URL: https, ssh, or a local path.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub git: Option<String>,
    /// The full commit the plugin is pinned at, with `git`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rev: Option<String>,
    /// A local directory, without `git`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// The plugin's directory within the source, when it is not the root.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dir: Option<String>,
    /// The SHA-256 of every file it ships, as trust hashes a directory.
    pub digest: String,
}

impl Pin {
    /// Where the source is, for a person.
    pub(crate) fn source(&self) -> String {
        let at = self
            .dir
            .as_deref()
            .map(|d| format!(" ({d})"))
            .unwrap_or_default();
        match (&self.git, &self.rev, &self.path) {
            (Some(git), Some(rev), _) => format!("{git} at {}{at}", short(rev)),
            (Some(git), None, _) => format!("{git}{at}"),
            (None, _, Some(path)) => format!("{path}{at}"),
            (None, _, None) => String::new(),
        }
    }

    /// What is wrong with the pin itself, before anything is read for it.
    pub(crate) fn problem(&self) -> Option<String> {
        if !is_hex(&self.digest, &[64]) {
            return Some("its `digest` is not a SHA-256 in hex".into());
        }
        if let Some(dir) = &self.dir
            && !is_inside(dir)
        {
            return Some(format!(
                "its `dir` must be a path inside the source, not `{dir}`"
            ));
        }
        match (&self.git, &self.rev, &self.path) {
            (Some(_), Some(rev), None) if is_hex(rev, &[40, 64]) => None,
            (Some(_), _, None) => {
                Some("a git plugin is pinned by `rev`, a full commit hash".into())
            }
            (None, None, Some(_)) => None,
            _ => Some("a plugin names either `git` and `rev`, or `path`".into()),
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

/// Whether a relative path stays inside the directory it is joined to.
fn is_inside(path: &str) -> bool {
    let path = Path::new(path);
    !path.is_absolute()
        && path
            .components()
            .all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
}

/// Reads a `plugins.toml`; a missing one pins nothing.
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
        "# Plugins, pinned by `jouet plugin add`; see https://jouet.jdx.dev/plugins\n{text}"
    );
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
    }
    std::fs::write(path, text).map_err(|e| format!("writing {}: {e}", path.display()))
}

/// `<state dir>/plugins`, where fetched plugins are kept by their hash.
fn store() -> Option<PathBuf> {
    jouet_output::paths::state_dir().map(|d| d.join("plugins"))
}

/// Where a pin's files are kept.
pub(crate) fn stored(pin: &Pin) -> Option<PathBuf> {
    store().map(|s| s.join(&pin.digest))
}

/// A fetched plugin: its manifest, the pin to record, and where it is kept.
pub(crate) struct Fetched {
    pub manifest: Manifest,
    pub pin: Pin,
    pub root: PathBuf,
}

/// Whether a source names a git repository rather than a directory.
pub(crate) fn is_git_url(source: &str) -> bool {
    source.contains("://")
        || (source.contains('@') && source.contains(':') && !Path::new(source).exists())
        || source.ends_with(".git")
}

/// Fetches a plugin: a git repository at `rev` (its default branch when
/// none), or a directory, keeping its files in the store. With `want`, they
/// must hash to it.
pub(crate) fn fetch(
    source: &str,
    git: bool,
    rev: Option<&str>,
    dir: Option<&str>,
    want: Option<&str>,
) -> Result<Fetched, String> {
    if let Some(dir) = dir
        && !is_inside(dir)
    {
        return Err(format!(
            "--dir must be a path inside the source, not `{dir}`"
        ));
    }
    let store = store()
        .ok_or("there is no home directory to keep plugins in; set HOME or XDG_STATE_HOME")?;
    std::fs::create_dir_all(&store).map_err(|e| format!("creating {}: {e}", store.display()))?;
    let staging = tempfile::Builder::new()
        .prefix(".fetch-")
        .tempdir_in(&store)
        .map_err(|e| format!("creating a directory in {}: {e}", store.display()))?;
    let files = staging.path().join("plugin");
    std::fs::create_dir(&files).map_err(|e| e.to_string())?;
    let commit = if git {
        Some(fetch_git(source, rev, dir, staging.path(), &files)?)
    } else {
        let from = match dir {
            Some(dir) => Path::new(source).join(dir),
            None => PathBuf::from(source),
        };
        copy_dir(&from, &files)?;
        None
    };
    let manifest = Manifest::read(&files)?;
    if let Some(need) = manifest.needs() {
        return Err(format!(
            "{} needs jouet {need} or newer; this is {}",
            manifest.name,
            env!("CARGO_PKG_VERSION")
        ));
    }
    if !KINDS.iter().any(|k| files.join(k).is_dir()) {
        return Err(format!(
            "{} ships no rules/, routines/ or formats/",
            manifest.name
        ));
    }
    let digest = Digests::load(None)
        .of(&files, ENTRIES)
        .map_err(|e| format!("hashing the plugin: {e}"))?;
    if let Some(want) = want
        && want != digest
    {
        return Err(format!(
            "the files fetched from {source} hash to {digest}, not the pinned {want}"
        ));
    }
    let kept = store.join(&digest);
    // Kept before: the same files, unless something changed them since.
    if kept.is_dir()
        && Digests::load(None).of(&kept, ENTRIES).ok().as_deref() != Some(digest.as_str())
    {
        std::fs::remove_dir_all(&kept).map_err(|e| format!("replacing {}: {e}", kept.display()))?;
    }
    if !kept.is_dir() {
        std::fs::rename(&files, &kept)
            .map_err(|e| format!("keeping the plugin in {}: {e}", kept.display()))?;
    }
    let pin = Pin {
        git: git.then(|| source.to_string()),
        rev: commit,
        path: (!git).then(|| {
            std::path::absolute(source)
                .map_or_else(|_| source.to_string(), |p| p.display().to_string())
        }),
        dir: dir.map(str::to_string),
        digest,
    };
    Ok(Fetched {
        manifest,
        pin,
        root: kept,
    })
}

/// Fetches a pinned plugin whose files are missing from the store or do
/// not match its pin. Returns whether it fetched.
pub(crate) fn sync(pin: &Pin, digests: &mut Digests) -> Result<bool, String> {
    if let Some(problem) = pin.problem() {
        return Err(problem);
    }
    if let Some(kept) = stored(pin)
        && digests.of(&kept, ENTRIES).ok().as_deref() == Some(pin.digest.as_str())
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
        pin.dir.as_deref(),
        Some(&pin.digest),
    )?;
    Ok(true)
}

/// What a file at `path`, relative to the plugin's root, is to the plugin:
/// one to keep, one to leave out, or one that makes it no plugin.
enum Entry {
    Keep,
    Skip,
}

fn classify(path: &str) -> Result<Entry, String> {
    let mut parts = path.split('/');
    let top = parts.next().unwrap_or_default();
    let nested = parts.next().is_some();
    if !nested {
        // A file at the root: the manifest, or a README and the like.
        return Ok(if top == MANIFEST {
            Entry::Keep
        } else {
            Entry::Skip
        });
    }
    if KINDS.contains(&top) {
        return Ok(Entry::Keep);
    }
    if top.starts_with('.') {
        return Ok(Entry::Skip);
    }
    Err(format!(
        "{top}/ is not something a plugin ships; it holds {} only, and {MANIFEST}",
        KINDS
            .iter()
            .map(|k| format!("{k}/"))
            .collect::<Vec<_>>()
            .join(", ")
    ))
}

/// Copies a directory's plugin files into `to`.
fn copy_dir(from: &Path, to: &Path) -> Result<(), String> {
    if !from.is_dir() {
        return Err(format!("{} is not a directory", from.display()));
    }
    let mut paths = Vec::new();
    list(from, "", &mut paths)?;
    for (rel, path) in paths {
        if let Entry::Skip = classify(&rel)? {
            continue;
        }
        let meta = std::fs::symlink_metadata(&path)
            .map_err(|e| format!("reading {}: {e}", path.display()))?;
        if !meta.is_file() {
            return Err(format!(
                "{rel} is not a plain file; a plugin ships plain files only"
            ));
        }
        let target = to.join(&rel);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::copy(&path, &target).map_err(|e| format!("copying {}: {e}", path.display()))?;
    }
    Ok(())
}

/// Every entry below `dir` that is not a directory, named from the root.
fn list(dir: &Path, prefix: &str, out: &mut Vec<(String, PathBuf)>) -> Result<(), String> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| format!("reading {}: {e}", dir.display()))?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .collect();
    entries.sort();
    for path in entries {
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        let rel = if prefix.is_empty() {
            name.to_string()
        } else {
            format!("{prefix}/{name}")
        };
        let is_dir = std::fs::symlink_metadata(&path).is_ok_and(|m| m.is_dir());
        if is_dir {
            // Hidden directories at the root are the source's, not the plugin's.
            if prefix.is_empty() && name.starts_with('.') {
                continue;
            }
            list(&path, &rel, out)?;
        } else {
            out.push((rel, path));
        }
    }
    Ok(())
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
/// `scratch`, checks the commit, and writes the plugin's files of its tree
/// to `to`. Returns the full commit.
fn fetch_git(
    url: &str,
    rev: Option<&str>,
    dir: Option<&str>,
    scratch: &Path,
    to: &Path,
) -> Result<String, String> {
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
    let mut tree = git(&repo);
    tree.args(["ls-tree", "-r", "-z", "--full-tree", &commit]);
    if let Some(dir) = dir {
        tree.args(["--", dir]);
    }
    let listing = run(
        tree,
        &format!(
            "listing {} at {}",
            dir.unwrap_or("the tree"),
            short(&commit)
        ),
    )?;
    let prefix = dir.map(|d| format!("{}/", d.trim_end_matches('/')));
    for entry in listing.split(|b| *b == 0).filter(|e| !e.is_empty()) {
        let entry = String::from_utf8_lossy(entry);
        let Some((meta, path)) = entry.split_once('\t') else {
            continue;
        };
        let mut meta = meta.split(' ');
        let (Some(mode), Some(kind), Some(oid)) = (meta.next(), meta.next(), meta.next()) else {
            continue;
        };
        let rel = match &prefix {
            Some(prefix) => match path.strip_prefix(prefix.as_str()) {
                Some(rel) => rel,
                None => continue,
            },
            None => path,
        };
        if let Entry::Skip = classify(rel)? {
            continue;
        }
        if kind != "blob" || !matches!(mode, "100644" | "100755") || !is_inside(rel) {
            return Err(format!(
                "{rel} is not a plain file; a plugin ships plain files only"
            ));
        }
        let mut blob = git(&repo);
        blob.args(["cat-file", "blob", oid]);
        let bytes = run(blob, &format!("reading {path}"))?;
        let target = to.join(rel);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::write(&target, bytes).map_err(|e| format!("writing {}: {e}", target.display()))?;
    }
    Ok(commit)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pin() -> Pin {
        Pin {
            git: Some("https://example.com/r.git".into()),
            rev: Some("a".repeat(40)),
            path: None,
            dir: None,
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
            dir: Some("../x".into()),
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
            plugin: BTreeMap::from([("team".to_string(), pin())]),
        };
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("plugins.toml");
        write(&path, &pins).unwrap();
        assert_eq!(read(&path).unwrap().plugin, pins.plugin);
        assert!(
            read(&dir.path().join("none.toml"))
                .unwrap()
                .plugin
                .is_empty()
        );
        std::fs::write(&path, "[pack.x]\n").unwrap();
        assert!(read(&path).unwrap_err().contains("unknown field `pack`"));
    }

    #[test]
    fn git_urls_are_told_from_paths() {
        assert!(is_git_url("https://github.com/acme/rules"));
        assert!(is_git_url("git@github.com:acme/rules.git"));
        assert!(!is_git_url("../team-rules"));
    }

    #[test]
    fn a_plugin_ships_rules_routines_and_its_manifest_only() {
        assert!(matches!(classify(MANIFEST), Ok(Entry::Keep)));
        assert!(matches!(classify("rules/a.toml"), Ok(Entry::Keep)));
        assert!(matches!(
            classify("routines/fixtures/a.json"),
            Ok(Entry::Keep)
        ));
        assert!(matches!(classify("README.md"), Ok(Entry::Skip)));
        assert!(matches!(
            classify(".github/workflows/ci.yml"),
            Ok(Entry::Skip)
        ));
        let unknown = classify("skills/x.md").err().unwrap();
        assert!(
            unknown.starts_with("skills/ is not something a plugin ships"),
            "{unknown}"
        );
    }

    #[test]
    fn a_manifest_names_the_plugin_and_the_oldest_jouet() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            Manifest::read(dir.path()).unwrap_err(),
            "it has no jouet-plugin.toml"
        );
        let write = |text: &str| std::fs::write(dir.path().join(MANIFEST), text).unwrap();
        write("name = \"acme\"\ndescription = \"the team's\"\n");
        let manifest = Manifest::read(dir.path()).unwrap();
        assert_eq!((manifest.name.as_str(), manifest.needs()), ("acme", None));
        write("name = \"acme\"\njouet = \"999.0\"\n");
        assert_eq!(Manifest::read(dir.path()).unwrap().needs(), Some("999.0"));
        write("name = \"acme\"\nskills = []\n");
        assert!(
            Manifest::read(dir.path())
                .unwrap_err()
                .contains("unknown field `skills`")
        );
        write("name = \"Acme Rules\"\n");
        assert!(
            Manifest::read(dir.path())
                .unwrap_err()
                .contains("lowercase words")
        );
        write("name = \"acme\"\njouet = \"soon\"\n");
        assert!(
            Manifest::read(dir.path())
                .unwrap_err()
                .contains("oldest jouet")
        );
    }
}

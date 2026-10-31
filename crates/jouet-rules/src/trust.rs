//! Which directories' rules may run.
//!
//! A repository's `.jouet/rules/` can deny commands and put text in front of
//! the agent, so it runs only once the user has trusted it. Trust is recorded
//! against a hash of every rule file in the directory: adding or changing a
//! rule needs trusting again, like a reviewed lockfile. A directory keeps
//! every hash it was trusted at, so going back to a branch already trusted
//! needs no trusting again.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trust {
    Trusted,
    /// Trusted once, but the rules have changed since.
    Changed,
    Untrusted,
}

/// How many hashes a directory keeps: enough for the branches in use.
const KEEP: usize = 32;

pub struct TrustStore {
    /// None when there is nowhere to keep trust: nothing is trusted, and
    /// trusting fails.
    path: Option<PathBuf>,
    /// Canonical directory → the hashes of its rules it was trusted at.
    dirs: BTreeMap<String, Vec<String>>,
}

impl TrustStore {
    /// Reads the store; a missing or unreadable file trusts nothing.
    pub fn load(path: Option<PathBuf>) -> Self {
        let dirs = path
            .as_ref()
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Self { path, dirs }
    }

    /// Whether the directory's rules, as they are now, are trusted.
    pub fn check(&self, dir: &Path) -> Trust {
        match digest(dir) {
            Ok(current) => self.check_digest(dir, &current),
            Err(_) if self.dirs.contains_key(&key(dir)) => Trust::Changed,
            Err(_) => Trust::Untrusted,
        }
    }

    /// Whether the directory was trusted when its rules hashed to `digest`.
    pub fn check_digest(&self, dir: &Path, digest: &str) -> Trust {
        match self.dirs.get(&key(dir)) {
            None => Trust::Untrusted,
            Some(trusted) if trusted.iter().any(|d| d == digest) => Trust::Trusted,
            Some(_) => Trust::Changed,
        }
    }

    /// Trusts the directory's rules as they are now.
    pub fn trust(&mut self, dir: &Path) -> std::io::Result<()> {
        let digest = digest(dir)?;
        self.trust_digest(dir, digest)
    }

    /// Trusts the directory at the hash its rules have now, kept with the
    /// hashes it was trusted at before.
    pub fn trust_digest(&mut self, dir: &Path, digest: String) -> std::io::Result<()> {
        let digests = self.dirs.entry(key(dir)).or_default();
        digests.retain(|d| *d != digest);
        digests.push(digest);
        let excess = digests.len().saturating_sub(KEEP);
        digests.drain(..excess);
        self.save()
    }

    /// Returns whether the directory was trusted.
    pub fn untrust(&mut self, dir: &Path) -> std::io::Result<bool> {
        let removed = self.dirs.remove(&key(dir)).is_some();
        if removed {
            self.save()?;
        }
        Ok(removed)
    }

    /// Every trusted directory, and how many hashes it was trusted at.
    pub fn entries(&self) -> impl Iterator<Item = (&str, usize)> {
        self.dirs.iter().map(|(dir, d)| (dir.as_str(), d.len()))
    }

    fn save(&self) -> std::io::Result<()> {
        let Some(path) = &self.path else {
            return Err(std::io::Error::other(
                "there is no home directory to keep trust in; set HOME or XDG_STATE_HOME",
            ));
        };
        write_atomic(path, &serde_json::to_vec_pretty(&self.dirs)?)
    }
}

pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

/// The key a directory is trusted under.
pub(crate) fn key(dir: &Path) -> String {
    dir.canonicalize()
        .unwrap_or_else(|_| dir.to_path_buf())
        .display()
        .to_string()
}

/// A hash over the paths and contents of the directory's rule files and
/// the scripts beside or below them.
pub(crate) fn digest(dir: &Path) -> std::io::Result<String> {
    hash(&set_files(dir, None)?)
}

/// The files a set of rules is hashed over: every `.toml` and `.luau` file
/// in `dir` and below it, named by its path from `dir`, and the pack pins
/// in `packs` when that file exists, named `../packs.toml` so no rule file
/// can take its place.
pub(crate) fn set_files(
    dir: &Path,
    packs: Option<&Path>,
) -> std::io::Result<Vec<(String, PathBuf)>> {
    let mut out: Vec<(String, PathBuf)> = files(dir)?
        .into_iter()
        .map(|path| {
            let name = path
                .strip_prefix(dir)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            (name, path)
        })
        .collect();
    if let Some(packs) = packs.filter(|p| p.is_file()) {
        out.push(("../packs.toml".into(), packs.to_path_buf()));
    }
    Ok(out)
}

fn hash(files: &[(String, PathBuf)]) -> std::io::Result<String> {
    let mut hasher = Sha256::new();
    for (name, path) in files {
        let code = std::fs::read(path)?;
        hasher.update(name.len().to_le_bytes());
        hasher.update(name.as_bytes());
        hasher.update(code.len().to_le_bytes());
        hasher.update(&code);
    }
    Ok(hex(&hasher.finalize()))
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Every `.toml` and `.luau` file in the directory and below it, sorted.
fn files(dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut out = crate::rule_files(dir)?;
    let mut entries: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(entries) => entries.filter_map(Result::ok).map(|e| e.path()).collect(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(e),
    };
    entries.sort();
    for entry in entries {
        if entry.is_dir() {
            out.extend(files(&entry)?);
        }
    }
    Ok(out)
}

/// A file as `stat` sees it: when any of this changes, it is hashed again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Stat {
    name: String,
    size: u64,
    mtime_ns: u128,
    /// The inode change time and inode, on Unix: unlike the modification
    /// time, a write cannot set them back.
    ctime_ns: i128,
    ino: u64,
}

impl Stat {
    fn of(name: &str, path: &Path) -> std::io::Result<Self> {
        let meta = std::fs::metadata(path)?;
        let mtime_ns = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_nanos());
        #[cfg(unix)]
        let (ctime_ns, ino) = {
            use std::os::unix::fs::MetadataExt;
            (
                i128::from(meta.ctime()) * 1_000_000_000 + i128::from(meta.ctime_nsec()),
                meta.ino(),
            )
        };
        #[cfg(not(unix))]
        let (ctime_ns, ino) = (0, 0);
        Ok(Self {
            name: name.to_string(),
            size: meta.len(),
            mtime_ns,
            ctime_ns,
            ino,
        })
    }
}

impl Stat {
    /// When the file last changed, as far as its times say.
    fn changed_ns(&self) -> u128 {
        self.mtime_ns
            .max(u128::try_from(self.ctime_ns).unwrap_or(0))
    }
}

#[derive(Serialize, Deserialize)]
struct Cached {
    files: Vec<Stat>,
    digest: String,
}

/// Hashes of rule sets, cached by each file's size, times and inode, so a
/// hook hashes a set again only after a file in it changed. The cache lives
/// where only the user writes, like the compiled rules.
pub struct Digests {
    path: Option<PathBuf>,
    sets: BTreeMap<String, Cached>,
    dirty: bool,
    /// A file changed this recently is not cached: a filesystem's clock can
    /// be coarse enough that a second write within the same tick leaves
    /// every time as it was.
    racy: std::time::Duration,
}

impl Digests {
    /// Reads the cache; a missing or unreadable one is empty.
    pub fn load(path: Option<PathBuf>) -> Self {
        let sets = path
            .as_ref()
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Self {
            path,
            sets,
            dirty: false,
            racy: std::time::Duration::from_secs(1),
        }
    }

    /// The hash of the rules in `dir` and the pins in `packs`, from the
    /// cache when no file has changed since it was taken.
    pub fn of(&mut self, dir: &Path, packs: Option<&Path>) -> std::io::Result<String> {
        let files = set_files(dir, packs)?;
        let stats = files
            .iter()
            .map(|(name, path)| Stat::of(name, path))
            .collect::<std::io::Result<Vec<_>>>()?;
        let key = key(dir);
        if let Some(cached) = self.sets.get(&key)
            && cached.files == stats
        {
            return Ok(cached.digest.clone());
        }
        let digest = hash(&files)?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let settled = now.saturating_sub(self.racy.as_nanos());
        if stats.iter().any(|s| s.changed_ns() >= settled) {
            self.sets.remove(&key);
            return Ok(digest);
        }
        self.sets.insert(
            key,
            Cached {
                files: stats,
                digest: digest.clone(),
            },
        );
        self.dirty = true;
        Ok(digest)
    }

    /// Writes the cache back if a hash was taken; failing to is harmless.
    pub fn save(&self) {
        if let (true, Some(path)) = (self.dirty, &self.path)
            && let Ok(bytes) = serde_json::to_vec(&self.sets)
        {
            let _ = write_atomic(path, &bytes);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trust_follows_the_rules_contents() {
        let dir = tempfile::tempdir().unwrap();
        let rules = dir.path().join("rules");
        std::fs::create_dir(&rules).unwrap();
        std::fs::write(rules.join("a.luau"), "return {}").unwrap();
        let mut store = TrustStore::load(Some(dir.path().join("trust.json")));
        assert_eq!(store.check(&rules), Trust::Untrusted);

        store.trust(&rules).unwrap();
        assert_eq!(store.check(&rules), Trust::Trusted);
        let reloaded = TrustStore::load(Some(dir.path().join("trust.json")));
        assert_eq!(reloaded.check(&rules), Trust::Trusted);

        std::fs::write(rules.join("b.luau"), "return {}").unwrap();
        assert_eq!(store.check(&rules), Trust::Changed);
        store.trust(&rules).unwrap();
        std::fs::write(rules.join("c.toml"), "").unwrap();
        assert_eq!(store.check(&rules), Trust::Changed);
        store.trust(&rules).unwrap();
        std::fs::create_dir(rules.join("lib")).unwrap();
        std::fs::write(rules.join("lib").join("d.luau"), "return {}").unwrap();
        assert_eq!(store.check(&rules), Trust::Changed);

        // Back to rules trusted before, as on switching branches.
        std::fs::remove_dir_all(rules.join("lib")).unwrap();
        assert_eq!(store.check(&rules), Trust::Trusted);

        assert!(store.untrust(&rules).unwrap());
        assert_eq!(store.check(&rules), Trust::Untrusted);
    }

    #[test]
    fn the_pack_pins_beside_a_directory_count_toward_its_hash() {
        let dir = tempfile::tempdir().unwrap();
        let rules = dir.path().join("rules");
        std::fs::create_dir(&rules).unwrap();
        let packs = dir.path().join("packs.toml");
        let mut digests = Digests::load(Some(dir.path().join("digests.json")));
        let bare = digests.of(&rules, Some(&packs)).unwrap();
        std::fs::write(&packs, "[pack.x]\n").unwrap();
        let pinned = digests.of(&rules, Some(&packs)).unwrap();
        assert_ne!(bare, pinned);
        // A rule file of that name is not the pins.
        std::fs::remove_file(&packs).unwrap();
        std::fs::write(rules.join("packs.toml"), "[pack.x]\n").unwrap();
        assert_ne!(digests.of(&rules, Some(&packs)).unwrap(), pinned);
    }

    #[test]
    fn a_cached_hash_is_taken_again_after_a_change() {
        let dir = tempfile::tempdir().unwrap();
        let rules = dir.path().join("rules");
        std::fs::create_dir(&rules).unwrap();
        std::fs::write(rules.join("a.toml"), "one").unwrap();
        let cache = dir.path().join("digests.json");
        let mut digests = Digests::load(Some(cache.clone()));
        let first = digests.of(&rules, None).unwrap();
        assert_eq!(first, digest(&rules).unwrap());
        digests.save();

        // Just written, so not cached yet.
        assert!(!digests.dirty);

        let mut digests = Digests::load(Some(cache.clone()));
        digests.racy = std::time::Duration::ZERO;
        assert_eq!(digests.of(&rules, None).unwrap(), first);
        assert!(digests.dirty);
        digests.save();
        let mut digests = Digests::load(Some(cache));
        digests.racy = std::time::Duration::ZERO;
        assert_eq!(digests.of(&rules, None).unwrap(), first);
        assert!(!digests.dirty);
        std::fs::write(rules.join("a.toml"), "three").unwrap();
        let second = digests.of(&rules, None).unwrap();
        assert_ne!(second, first);
        assert_eq!(second, digest(&rules).unwrap());
    }

    #[test]
    fn without_a_place_to_keep_it_nothing_is_trusted() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = TrustStore::load(None);
        assert_eq!(store.check(dir.path()), Trust::Untrusted);
        assert!(store.trust(dir.path()).is_err());
    }
}

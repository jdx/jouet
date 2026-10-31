//! What `ci explain` fetched from GitHub, kept under the state dir so that
//! explaining the same failure again costs a few requests rather than a
//! few dozen.
//!
//! One file per response, named after its API path, in a directory per
//! repository: `<state>/ci/<owner>-<name>/actions.runs.1.json`. A file's
//! modification time is when it was fetched, and each read says how old an
//! answer it accepts, so a finished job's log is read for a month while a
//! list of recent runs is fetched again after five minutes. Nothing stays
//! longer than [`MAX_AGE`], and the whole cache stays under [`MAX_BYTES`],
//! oldest files going first; both are checked when the cache is written.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// The oldest file the cache keeps.
pub(crate) const MAX_AGE: Duration = Duration::from_secs(30 * 24 * 60 * 60);
/// The most the cache holds, all repositories together.
pub(crate) const MAX_BYTES: u64 = 64 * 1024 * 1024;

pub(crate) struct Cache {
    /// `<state>/ci`, or `None` without a home directory: nothing is kept.
    root: Option<PathBuf>,
    /// This repository's directory under `root`.
    dir: Option<PathBuf>,
    /// What the cache held at the last scan, plus what was written since;
    /// `None` until the first write scans it.
    size: Option<u64>,
    max_bytes: u64,
}

impl Cache {
    /// The cache for `owner/name` under `root`.
    pub(crate) fn new(root: Option<PathBuf>, owner: &str, name: &str) -> Self {
        let dir = root.as_ref().map(|r| r.join(format!("{owner}-{name}")));
        Self {
            root,
            dir,
            size: None,
            max_bytes: MAX_BYTES,
        }
    }

    /// The file a key is kept in, whether or not it exists.
    pub(crate) fn path(&self, key: &str) -> Option<PathBuf> {
        self.dir.as_ref().map(|d| d.join(key))
    }

    /// The text kept under `key`, if it was fetched less than `max_age` ago.
    pub(crate) fn read(&self, key: &str, max_age: Duration) -> Option<String> {
        let path = self.path(key)?;
        (age(&path)? < max_age.min(MAX_AGE))
            .then(|| fs::read_to_string(&path).ok())
            .flatten()
    }

    /// Keeps `text` under `key`, then prunes. A cache that cannot be written
    /// only costs the next run a request, so a failure is ignored.
    pub(crate) fn write(&mut self, key: &str, text: &str) {
        let (Some(dir), Some(path)) = (self.dir.clone(), self.path(key)) else {
            return;
        };
        let written = fs::create_dir_all(&dir).and_then(|()| {
            let mut tmp = tempfile::NamedTempFile::new_in(&dir)?;
            std::io::Write::write_all(&mut tmp, text.as_bytes())?;
            tmp.persist(&path).map_err(|e| e.error)?;
            Ok(())
        });
        if written.is_err() {
            return;
        }
        match &mut self.size {
            Some(size) => {
                *size += text.len() as u64;
                if *size > self.max_bytes {
                    self.prune();
                }
            }
            None => self.prune(),
        }
    }

    /// Drops `key`, as after a rerun makes a run's document stale.
    pub(crate) fn forget(&self, key: &str) {
        if let Some(path) = self.path(key) {
            let _ = fs::remove_file(path);
        }
    }

    /// Removes files older than [`MAX_AGE`], then the oldest until the
    /// cache fits in its size limit.
    fn prune(&mut self) {
        let Some(root) = &self.root else {
            return;
        };
        let mut files = Vec::new();
        for repo in fs::read_dir(root).into_iter().flatten().flatten() {
            for file in fs::read_dir(repo.path()).into_iter().flatten().flatten() {
                let Ok(meta) = file.metadata() else {
                    continue;
                };
                if !meta.is_file() {
                    continue;
                }
                let modified = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                files.push((modified, meta.len(), file.path()));
            }
        }
        let now = SystemTime::now();
        files.retain(|(modified, _, path)| {
            let old = now.duration_since(*modified).unwrap_or_default() > MAX_AGE;
            if old {
                let _ = fs::remove_file(path);
            }
            !old
        });
        files.sort();
        let mut size: u64 = files.iter().map(|(_, len, _)| len).sum();
        for (_, len, path) in &files {
            if size <= self.max_bytes {
                break;
            }
            if fs::remove_file(path).is_ok() {
                size -= len;
            }
        }
        self.size = Some(size);
    }
}

/// How long ago a file was written; `None` when it does not exist.
fn age(path: &Path) -> Option<Duration> {
    let modified = fs::metadata(path).ok()?.modified().ok()?;
    Some(
        SystemTime::now()
            .duration_since(modified)
            .unwrap_or_default(),
    )
}

/// The file name a REST path is kept under: the path after
/// `/repos/OWNER/NAME/`, with `/` as `.` and the query's `?` and `&` as
/// `@`, as in `actions.runs.1.attempts.1.jobs@per_page=100.json`.
pub(crate) fn key(path: &str) -> String {
    let rest = path
        .strip_prefix("/repos/")
        .and_then(|p| p.splitn(3, '/').nth(2))
        .unwrap_or("repository");
    let rest = if rest.is_empty() { "repository" } else { rest };
    let name: String = rest
        .chars()
        .map(|c| match c {
            '/' => '.',
            '?' | '&' => '@',
            c if c.is_ascii_alphanumeric() || "-_.=,".contains(c) => c,
            _ => '_',
        })
        .collect();
    format!("{name}.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_name_the_path_within_the_repository() {
        assert_eq!(
            key("/repos/jdx/jouet/actions/runs/1/attempts/2/jobs?per_page=100"),
            "actions.runs.1.attempts.2.jobs@per_page=100.json"
        );
        assert_eq!(
            key("/repos/jdx/jouet/actions/runs?branch=main&per_page=30"),
            "actions.runs@branch=main@per_page=30.json"
        );
        assert_eq!(key("/repos/jdx/jouet"), "repository.json");
    }

    fn set_age(path: &Path, age: Duration) {
        let file = fs::File::options().write(true).open(path).unwrap();
        file.set_modified(SystemTime::now() - age).unwrap();
    }

    #[test]
    fn reads_honour_their_age_and_old_files_are_pruned_on_write() {
        let dir = tempfile::tempdir().unwrap();
        let mut cache = Cache::new(Some(dir.path().to_path_buf()), "o", "r");
        cache.write("a.json", "a");
        assert_eq!(
            cache.read("a.json", Duration::from_secs(60)).as_deref(),
            Some("a")
        );
        set_age(&cache.path("a.json").unwrap(), Duration::from_secs(120));
        assert_eq!(cache.read("a.json", Duration::from_secs(60)), None);
        assert!(cache.read("a.json", MAX_AGE).is_some());

        // Another repository's file past the limit goes on the next scan.
        let mut other = Cache::new(Some(dir.path().to_path_buf()), "o", "other");
        other.write("b.json", "b");
        set_age(
            &other.path("b.json").unwrap(),
            MAX_AGE + Duration::from_secs(1),
        );
        let mut fresh = Cache::new(Some(dir.path().to_path_buf()), "o", "r");
        fresh.write("c.json", "c");
        assert!(!other.path("b.json").unwrap().exists());
        assert!(fresh.path("a.json").unwrap().exists());
    }

    #[test]
    fn the_oldest_files_go_first_when_the_cache_is_full() {
        let dir = tempfile::tempdir().unwrap();
        let mut cache = Cache::new(Some(dir.path().to_path_buf()), "o", "r");
        cache.max_bytes = 10;
        cache.write("old.json", "12345");
        set_age(&cache.path("old.json").unwrap(), Duration::from_secs(60));
        cache.write("mid.json", "12345");
        set_age(&cache.path("mid.json").unwrap(), Duration::from_secs(30));
        cache.write("new.json", "12345");
        assert!(!cache.path("old.json").unwrap().exists());
        assert!(cache.path("mid.json").unwrap().exists());
        assert!(cache.path("new.json").unwrap().exists());
    }

    #[test]
    fn without_a_state_dir_nothing_is_kept() {
        let mut cache = Cache::new(None, "o", "r");
        cache.write("a.json", "a");
        assert_eq!(cache.read("a.json", MAX_AGE), None);
    }
}

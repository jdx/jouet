//! Keeping the store in step with the work tree, without a daemon.
//!
//! A refresh lists the work tree's source files (honouring `.gitignore`),
//! compares each one's size and modification time with what the store
//! remembers, hashes only the files whose metadata changed, and re-extracts
//! only the ones whose content changed. Extraction runs on a few threads;
//! writing is one transaction per batch.
//!
//! A refresh with a time budget stops starting new files once the budget is
//! spent and reports the rest as deferred: they are still stale, and the
//! next refresh picks them up.

use crate::extract::{self, Extraction};
use crate::store::{self, FileUpdate};
use crate::{Error, Graph};
use rusqlite::Connection;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant, UNIX_EPOCH};

/// codegraph's `MAX_SOURCE_FILE_SIZE_BYTES`: larger files are recorded but
/// not extracted (they are almost always generated or vendored).
pub(crate) const MAX_FILE_SIZE: u64 = 1024 * 1024;

/// Directories never indexed, even when nothing ignores them (codegraph's
/// `SUBPROJECT_SCAN_SKIP`, less the names that are often real source).
const SKIP_DIRS: [&str; 14] = [
    "node_modules",
    ".git",
    ".svn",
    ".hg",
    "vendor",
    ".next",
    ".nuxt",
    ".svelte-kit",
    ".cache",
    ".venv",
    "venv",
    "__pycache__",
    ".turbo",
    "target",
];

/// How many files an empty graph must be getting before it is filled in
/// bulk (see [`store::begin_bulk`]).
const BULK_FROM: usize = 32;

/// Files written per transaction, so a long index commits as it goes and
/// readers never wait long.
const BATCH: usize = 256;

/// What a refresh did.
#[derive(Debug, Default, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Refresh {
    /// Files compared with the store.
    pub checked: usize,
    /// Files extracted and written.
    pub indexed: usize,
    /// Files whose timestamp changed but content did not.
    pub touched: usize,
    /// Files that disappeared, and their symbols with them.
    pub removed: usize,
    /// Files recorded without symbols: larger than 1 MiB, or not UTF-8.
    pub skipped: usize,
    /// Stale files left for the next refresh because the budget ran out.
    pub deferred: usize,
    /// Files extraction could not walk, with why.
    pub failed: Vec<(String, String)>,
    #[serde(serialize_with = "millis")]
    pub elapsed: Duration,
}

fn millis<S: serde::Serializer>(d: &Duration, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_u64(d.as_millis() as u64)
}

/// A source file on disk.
struct Seen {
    rel: String,
    abs: PathBuf,
    size: u64,
    modified_ns: i64,
}

fn modified_ns(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos() as i64)
}

/// `path` relative to `root`, with `/` separators, or `None` outside it.
pub(crate) fn relative(root: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(root).ok()?;
    let parts: Vec<_> = rel
        .components()
        .map(|c| c.as_os_str().to_str())
        .collect::<Option<_>>()?;
    (!parts.is_empty()).then(|| parts.join("/"))
}

/// Every source file under `root` that jouet extracts.
fn list(root: &Path) -> Vec<Seen> {
    let walker = ignore::WalkBuilder::new(root)
        .hidden(true)
        .git_ignore(true)
        .git_global(false)
        .git_exclude(true)
        .require_git(false)
        .filter_entry(|e| {
            !(e.file_type().is_some_and(|t| t.is_dir())
                && e.file_name()
                    .to_str()
                    .is_some_and(|n| SKIP_DIRS.contains(&n)))
        })
        .build();
    let mut out = Vec::new();
    for entry in walker.flatten() {
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let Some(rel) = relative(root, entry.path()) else {
            continue;
        };
        if extract::language_for_path(&rel).is_none() {
            continue;
        }
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        out.push(Seen {
            rel,
            abs: entry.path().to_path_buf(),
            size: meta.len(),
            modified_ns: modified_ns(&meta),
        });
    }
    out
}

fn hash(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .take(16)
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// A file whose metadata changed, read and hashed.
struct Candidate {
    seen: Seen,
    hash: String,
    /// `None` when the file is too large to extract or is not UTF-8.
    source: Option<String>,
}

fn read(seen: Seen) -> Option<Candidate> {
    let bytes = std::fs::read(&seen.abs).ok()?;
    let hash = hash(&bytes);
    let source = (seen.size <= MAX_FILE_SIZE)
        .then(|| String::from_utf8(bytes).ok())
        .flatten();
    Some(Candidate { seen, hash, source })
}

fn extract_one(c: &Candidate) -> Option<(&'static str, Result<Extraction, String>)> {
    let Some(source) = &c.source else {
        let lang = extract::language_for_path(&c.seen.rel)?;
        let why = if c.seen.size > MAX_FILE_SIZE {
            "larger than 1 MiB"
        } else {
            "not UTF-8"
        };
        return Some((lang, Err(why.to_string())));
    };
    let lang = extract::detect_language(&c.seen.rel, source)?;
    Some((
        lang,
        extract::extract_on_this_thread(&c.seen.rel, source, lang),
    ))
}

impl Graph {
    /// Brings the whole store in step with the work tree. With a budget,
    /// stops starting new files once it is spent.
    pub fn refresh(&mut self, budget: Option<Duration>) -> Result<Refresh, Error> {
        let start = Instant::now();
        let seen = list(&self.root);
        let known = store::file_rows(&self.conn)?;
        let mut report = Refresh {
            checked: seen.len(),
            ..Refresh::default()
        };
        let on_disk: HashSet<&str> = seen.iter().map(|s| s.rel.as_str()).collect();
        let gone: Vec<String> = known
            .keys()
            .filter(|p| !on_disk.contains(p.as_str()))
            .cloned()
            .collect();
        let changed: Vec<Seen> = seen
            .into_iter()
            .filter(|s| {
                known
                    .get(&s.rel)
                    .is_none_or(|k| k.size != s.size || k.modified_ns != s.modified_ns)
            })
            .collect();
        {
            let tx = self.conn.transaction()?;
            for path in &gone {
                store::delete_file(&tx, path)?;
            }
            tx.commit()?;
        }
        report.removed = gone.len();
        let hashes: HashMap<String, String> = changed
            .iter()
            .filter_map(|s| {
                known
                    .get(&s.rel)
                    .map(|k| (s.rel.clone(), k.content_hash.clone()))
            })
            .collect();
        // Filling an empty graph is faster with its indexes built afterwards.
        let bulk = known.is_empty() && changed.len() > BULK_FROM;
        if bulk {
            store::begin_bulk(&self.conn)?;
        }
        let result = process(&mut self.conn, changed, &hashes, start, budget, &mut report);
        if bulk {
            store::end_bulk(&self.conn)?;
        }
        result?;
        report.elapsed = start.elapsed();
        Ok(report)
    }

    /// Drops the database and indexes the work tree from scratch.
    pub fn rebuild(&mut self) -> Result<Refresh, Error> {
        self.reopen_empty()?;
        self.refresh(None)
    }

    /// Re-indexes just these paths (absolute, or relative to the work
    /// tree), as a hook does after an edit: no walk of the work tree.
    /// Paths outside the work tree, ignored or in a language jouet does not
    /// extract are skipped; a deleted path drops its symbols.
    pub fn reindex_paths(
        &mut self,
        paths: &[PathBuf],
        budget: Option<Duration>,
    ) -> Result<Refresh, Error> {
        let start = Instant::now();
        let mut report = Refresh::default();
        let mut changed = Vec::new();
        let mut hashes = HashMap::new();
        let mut seen_paths = HashSet::new();
        for path in paths {
            let abs = if path.is_absolute() {
                path.clone()
            } else {
                self.root.join(path)
            };
            let Some(rel) = relative(&self.root, &abs) else {
                continue;
            };
            if extract::language_for_path(&rel).is_none()
                || is_skipped(&rel)
                || !seen_paths.insert(rel.clone())
            {
                continue;
            }
            report.checked += 1;
            let known = store::file_row(&self.conn, &rel)?;
            let Ok(meta) = std::fs::metadata(&abs) else {
                if known.is_some() {
                    let tx = self.conn.transaction()?;
                    store::delete_file(&tx, &rel)?;
                    tx.commit()?;
                    report.removed += 1;
                }
                continue;
            };
            let seen = Seen {
                rel,
                abs,
                size: meta.len(),
                modified_ns: modified_ns(&meta),
            };
            if let Some(k) = &known {
                if k.size == seen.size && k.modified_ns == seen.modified_ns {
                    continue;
                }
                hashes.insert(seen.rel.clone(), k.content_hash.clone());
            }
            changed.push(seen);
        }
        process(&mut self.conn, changed, &hashes, start, budget, &mut report)?;
        report.elapsed = start.elapsed();
        Ok(report)
    }
}

/// A path under a directory the walk never enters.
fn is_skipped(rel: &str) -> bool {
    rel.split('/').any(|part| SKIP_DIRS.contains(&part))
}

/// Reads, hashes, extracts and writes `changed`, honouring the budget.
fn process(
    conn: &mut Connection,
    changed: Vec<Seen>,
    hashes: &HashMap<String, String>,
    start: Instant,
    budget: Option<Duration>,
    report: &mut Refresh,
) -> Result<(), Error> {
    if changed.is_empty() {
        return Ok(());
    }
    let over = |now: Instant| budget.is_some_and(|b| now.duration_since(start) >= b);
    let workers = std::thread::available_parallelism()
        .map_or(4, |n| n.get())
        .clamp(1, 8)
        .min(changed.len());
    let queue = std::sync::Mutex::new(changed.into_iter());
    let (tx, rx) = mpsc::sync_channel::<Outcome>(BATCH);
    std::thread::scope(|scope| -> Result<(), Error> {
        for _ in 0..workers {
            let tx = tx.clone();
            let queue = &queue;
            std::thread::Builder::new()
                .stack_size(extract::STACK_SIZE)
                .spawn_scoped(scope, move || {
                    loop {
                        if over(Instant::now()) {
                            let left = queue.lock().map_or(0, |mut q| q.by_ref().count());
                            let _ = tx.send(Outcome::Deferred(left));
                            return;
                        }
                        let Some(seen) = queue.lock().ok().and_then(|mut q| q.next()) else {
                            return;
                        };
                        let outcome = match read(seen) {
                            None => Outcome::Unreadable,
                            Some(c) if hashes.get(&c.seen.rel) == Some(&c.hash) => {
                                Outcome::Touched(c.seen.rel, c.seen.modified_ns)
                            }
                            Some(c) => match extract_one(&c) {
                                None => Outcome::Unreadable,
                                Some((language, extraction)) => {
                                    let skipped = c.source.is_none();
                                    Outcome::Update(
                                        Box::new(FileUpdate {
                                            path: c.seen.rel,
                                            language,
                                            content_hash: c.hash,
                                            size: c.seen.size,
                                            modified_ns: c.seen.modified_ns,
                                            extraction,
                                        }),
                                        skipped,
                                    )
                                }
                            },
                        };
                        if tx.send(outcome).is_err() {
                            return;
                        }
                    }
                })
                .map_err(|e| Error(format!("spawning an indexing thread: {e}")))?;
        }
        drop(tx);
        let mut pending = Vec::with_capacity(BATCH);
        for outcome in rx {
            pending.push(outcome);
            if pending.len() >= BATCH {
                write(conn, &mut pending, report)?;
            }
        }
        write(conn, &mut pending, report)
    })
}

enum Outcome {
    /// A file to write; `true` when it was too large or not UTF-8 to read.
    Update(Box<FileUpdate>, bool),
    Touched(String, i64),
    Deferred(usize),
    Unreadable,
}

fn write(
    conn: &mut Connection,
    pending: &mut Vec<Outcome>,
    report: &mut Refresh,
) -> Result<(), Error> {
    if pending.is_empty() {
        return Ok(());
    }
    let tx = conn.transaction()?;
    for outcome in pending.drain(..) {
        match outcome {
            Outcome::Update(update, skipped) => {
                match &update.extraction {
                    Err(_) if skipped => report.skipped += 1,
                    Err(e) => report.failed.push((update.path.clone(), e.clone())),
                    Ok(_) => {}
                }
                store::write_file(&tx, &update)?;
                report.indexed += 1;
            }
            Outcome::Touched(path, modified_ns) => {
                store::touch_file(&tx, &path, modified_ns)?;
                report.touched += 1;
            }
            Outcome::Deferred(n) => report.deferred += n,
            Outcome::Unreadable => {}
        }
    }
    tx.commit()?;
    Ok(())
}

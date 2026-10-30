//! Finding and reading transcripts, in parallel.

mod claude;
mod codex;
mod json;

pub use claude::ClaudeCode;
pub use codex::Codex;

use crate::analyze::Analyzer;
use crate::event::Transcript;
use std::fs::File;
use std::io::{self, BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Instant, UNIX_EPOCH};

/// One harness's transcripts.
pub trait Reader: Sync {
    /// The harness's name in reports: `claude`, `codex`.
    fn harness(&self) -> &'static str;

    /// The transcript files that could match `filter`. A harness that is not
    /// installed has none.
    fn discover(&self, filter: &Filter) -> io::Result<Vec<Source>>;

    /// Reads one transcript, or `None` when it does not match `filter`'s
    /// project. A read error after the first line keeps what came before it
    /// and is reported as the transcript's `truncated`.
    fn read(
        &self,
        source: &Source,
        input: &mut dyn BufRead,
        filter: &Filter,
    ) -> io::Result<Option<Transcript>>;
}

/// Which sessions to read.
#[derive(Debug, Clone, Default)]
pub struct Filter {
    /// Only requests made at or after this time, in milliseconds since the
    /// Unix epoch.
    pub since_ms: Option<i64>,
    /// Only requests made before this time, in milliseconds since the Unix
    /// epoch: the history before `jouet setup`, for a baseline.
    pub until_ms: Option<i64>,
    /// Only sessions that ran in this directory or below it.
    pub project: Option<PathBuf>,
}

impl Filter {
    /// Whether a session's directory is the project or inside it.
    pub(crate) fn matches_cwd(&self, cwd: Option<&str>) -> bool {
        let Some(project) = &self.project else {
            return true;
        };
        let Some(cwd) = cwd else {
            return false;
        };
        let cwd = cwd.strip_prefix("file://").unwrap_or(cwd);
        Path::new(cwd).starts_with(project)
    }

    /// Whether a file last written at `modified_ms` can hold anything recent
    /// enough.
    fn wants(&self, source: &Source) -> bool {
        match (self.since_ms, source.modified_ms) {
            (Some(since), Some(modified)) => modified >= since,
            _ => true,
        }
    }
}

/// A transcript file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    pub path: PathBuf,
    pub bytes: u64,
    pub modified_ms: Option<i64>,
}

impl Source {
    pub fn from_path(path: PathBuf) -> io::Result<Self> {
        let meta = std::fs::metadata(&path)?;
        let modified_ms = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .and_then(|d| i64::try_from(d.as_millis()).ok());
        Ok(Self {
            path,
            bytes: meta.len(),
            modified_ms,
        })
    }
}

/// Opens a transcript, decompressing `.zst` files as they are read.
pub fn open(path: &Path) -> io::Result<Box<dyn BufRead>> {
    let file = File::open(path)?;
    if path.extension().is_some_and(|e| e == "zst") {
        Ok(Box::new(BufReader::with_capacity(
            1 << 16,
            zstd::stream::read::Decoder::new(file)?,
        )))
    } else {
        Ok(Box::new(BufReader::with_capacity(1 << 16, file)))
    }
}

/// The result of a scan.
#[derive(Debug)]
pub struct Scan {
    pub analyzer: Analyzer,
    pub stats: ScanStats,
}

#[derive(Debug, Clone, Default)]
pub struct ScanStats {
    /// Files read.
    pub files: u64,
    /// Their size on disk (compressed, for `.zst`).
    pub bytes: u64,
    /// Files left unread because they were last written before `--since`.
    pub skipped_old: u64,
    /// Files read whose session ran outside `--project`.
    pub other_projects: u64,
    /// Lines that were not valid JSON, skipped.
    pub bad_lines: u64,
    /// Files that could not be read, or were cut short by a read error.
    pub unreadable: Vec<Unreadable>,
    pub elapsed_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unreadable {
    pub path: PathBuf,
    pub error: String,
    /// What came before the error was still counted.
    pub partial: bool,
}

/// Reads every matching transcript of every reader into `analyzer`, adding
/// to whatever it already holds, on
/// `threads` threads (at least one). Larger files are started first, so one
/// large file does not finish last on its own.
pub fn scan(readers: &[&dyn Reader], filter: &Filter, analyzer: Analyzer, threads: usize) -> Scan {
    let started = Instant::now();
    let mut stats = ScanStats::default();
    let mut work: Vec<(usize, Source)> = Vec::new();
    for (i, reader) in readers.iter().enumerate() {
        match reader.discover(filter) {
            Ok(sources) => {
                for source in sources {
                    if filter.wants(&source) {
                        work.push((i, source));
                    } else {
                        stats.skipped_old += 1;
                    }
                }
            }
            Err(e) => stats.unreadable.push(Unreadable {
                path: PathBuf::from(reader.harness()),
                error: e.to_string(),
                partial: false,
            }),
        }
    }
    work.sort_by(|a, b| {
        b.1.bytes
            .cmp(&a.1.bytes)
            .then_with(|| a.1.path.cmp(&b.1.path))
    });

    let next = AtomicUsize::new(0);
    let results = Mutex::new(Vec::new());
    std::thread::scope(|s| {
        for _ in 0..threads.max(1).min(work.len().max(1)) {
            s.spawn(|| {
                // Settings only: what `analyzer` already holds is counted
                // once, in the merge below.
                let mut local = analyzer.empty_like();
                let mut local_stats = ScanStats::default();
                loop {
                    let n = next.fetch_add(1, Ordering::Relaxed);
                    let Some((i, source)) = work.get(n) else {
                        break;
                    };
                    let reader = readers[*i];
                    read_one(reader, source, filter, &mut local, &mut local_stats);
                }
                results
                    .lock()
                    .expect("no scan thread panics while holding the lock")
                    .push((local, local_stats));
            });
        }
    });
    let mut merged = analyzer;
    for (local, local_stats) in results.into_inner().unwrap_or_default() {
        merged.merge(local);
        stats.files += local_stats.files;
        stats.bytes += local_stats.bytes;
        stats.other_projects += local_stats.other_projects;
        stats.bad_lines += local_stats.bad_lines;
        stats.unreadable.extend(local_stats.unreadable);
    }
    stats.unreadable.sort_by(|a, b| a.path.cmp(&b.path));
    stats.elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    Scan {
        analyzer: merged,
        stats,
    }
}

fn read_one(
    reader: &dyn Reader,
    source: &Source,
    filter: &Filter,
    analyzer: &mut Analyzer,
    stats: &mut ScanStats,
) {
    let unreadable = |e: io::Error, partial| Unreadable {
        path: source.path.clone(),
        error: e.to_string(),
        partial,
    };
    let mut input = match open(&source.path) {
        Ok(input) => input,
        Err(e) => {
            stats.unreadable.push(unreadable(e, false));
            return;
        }
    };
    stats.files += 1;
    stats.bytes += source.bytes;
    match reader.read(source, &mut input, filter) {
        Ok(Some(mut transcript)) => {
            stats.bad_lines += transcript.bad_lines;
            if let Some(error) = transcript.truncated.take() {
                stats
                    .unreadable
                    .push(unreadable(io::Error::other(error), true));
            }
            if let Some(since) = filter.since_ms {
                transcript
                    .requests
                    .retain(|r| r.at.is_none_or(|at| at >= since));
            }
            if let Some(until) = filter.until_ms {
                transcript
                    .requests
                    .retain(|r| r.at.is_none_or(|at| at < until));
            }
            analyzer.add(reader.harness(), &transcript);
        }
        Ok(None) => stats.other_projects += 1,
        Err(e) => stats.unreadable.push(unreadable(e, false)),
    }
}

/// Calls `f` with each line of `input`, without its line ending. Stops at
/// the first read error and returns it.
pub(crate) fn lines(input: &mut dyn BufRead, mut f: impl FnMut(&[u8]) -> bool) -> Option<String> {
    let mut buf = Vec::with_capacity(1 << 16);
    loop {
        buf.clear();
        match input.read_until(b'\n', &mut buf) {
            Ok(0) => return None,
            Ok(_) => {
                let line = buf.strip_suffix(b"\n").unwrap_or(&buf);
                let line = line.strip_suffix(b"\r").unwrap_or(line);
                if !line.is_empty() && !f(line) {
                    return None;
                }
            }
            Err(e) => return Some(e.to_string()),
        }
    }
}

/// The user's home directory.
pub(crate) fn home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
}

/// Every file under `dir`, recursively, whose name ends with one of
/// `suffixes`. A missing directory has none.
pub(crate) fn files_under(dir: &Path, suffixes: &[&str], out: &mut Vec<PathBuf>) -> io::Result<()> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        let kind = entry.file_type()?;
        if kind.is_dir() {
            files_under(&path, suffixes, out)?;
        } else if path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| suffixes.iter().any(|s| n.ends_with(s)))
        {
            out.push(path);
        }
    }
    Ok(())
}

/// Sources for paths, leaving out any that vanished since they were listed.
pub(crate) fn sources(paths: Vec<PathBuf>) -> Vec<Source> {
    paths
        .into_iter()
        .filter_map(|p| Source::from_path(p).ok())
        .collect()
}

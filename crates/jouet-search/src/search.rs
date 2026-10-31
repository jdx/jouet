//! `jouet search`: ripgrep's walker and searcher, with the results grouped
//! by file and by enclosing symbol, cut to a byte budget with explicit
//! markers, and matches the session has already been shown collapsed.

use crate::memory::{self, Memory, Stamp};
use crate::structural::{Filter, Structural};
use crate::symbols;
use grep_regex::RegexMatcherBuilder;
use grep_searcher::sinks::Lossy;
use grep_searcher::{BinaryDetection, SearcherBuilder};
use ignore::WalkState;
use ignore::overrides::OverrideBuilder;
use serde::Serialize;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

/// How much of a matching line is shown.
const LINE_MAX: usize = 200;
/// How many files the closing marker names before counting the rest.
const OMITTED_NAMED: usize = 10;

/// What to search for, and where.
#[derive(Debug, Default)]
pub struct Options {
    pub pattern: String,
    /// Files and directories to search; the current directory when empty.
    pub paths: Vec<PathBuf>,
    pub fixed_strings: bool,
    pub ignore_case: bool,
    pub word: bool,
    /// Search hidden files and directories too.
    pub hidden: bool,
    /// Include or exclude files, as rg's `-g`.
    pub globs: Vec<String>,
    /// List the matching files and their counts, not the lines.
    pub files_only: bool,
    pub budget: Budget,
    /// Where each file's definitions are cached, by size and modification
    /// time; without one, every file is parsed afresh.
    pub cache: Option<PathBuf>,
    /// Match the pattern as code in this language, with ast-grep, rather
    /// than as a regular expression.
    pub structural: Option<String>,
    /// Narrows a structural pattern's matches, file by file.
    pub filter: Option<std::sync::Arc<dyn Filter>>,
}

/// Byte budgets for the output; `0` is no limit.
#[derive(Debug, Clone, Copy, Default)]
pub struct Budget {
    pub total: usize,
    pub per_file: usize,
}

/// A search's result.
#[derive(Debug, Default, Serialize)]
pub struct Report {
    pub files: Vec<FileReport>,
    /// Every match found, shown or not.
    pub matches: usize,
    pub files_matched: usize,
    pub files_searched: usize,
    /// Files with matches left out whole because the total budget ran out.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub omitted: Vec<Omitted>,
}

#[derive(Debug, Default, Serialize)]
pub struct FileReport {
    pub path: String,
    /// Every match in the file.
    pub matches: usize,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub shown: Vec<Hit>,
    /// Matches not shown because a budget ran out.
    #[serde(skip_serializing_if = "is_zero")]
    pub more: usize,
    /// Matches not shown because this session has already seen them, in
    /// the file as it is now.
    #[serde(skip_serializing_if = "is_zero")]
    pub already_shown: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct Hit {
    pub line: u32,
    pub text: String,
    /// The innermost definition the match is in.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol: Option<Symbol>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Symbol {
    pub kind: String,
    pub name: String,
    pub start: u32,
    pub end: u32,
}

#[derive(Debug, Serialize)]
pub struct Omitted {
    pub path: String,
    pub matches: usize,
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

/// A file's matches as the walker collected them.
struct Collected {
    path: PathBuf,
    key: String,
    stamp: Stamp,
    matches: usize,
    /// Matches not shown before, as many as could ever be shown.
    fresh: Vec<(u32, String)>,
    already_shown: usize,
}

/// Why a search could not run.
#[derive(Debug)]
pub enum Error {
    /// The pattern or a glob is invalid.
    Usage(String),
    /// A path to search does not exist.
    NotFound(PathBuf),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Usage(message) => f.write_str(message),
            Self::NotFound(path) => write!(f, "{}: no such file or directory", path.display()),
        }
    }
}

/// Runs a search, skipping matches `memory` says were already shown and
/// recording the ones shown now.
pub fn run(options: &Options, memory: &mut Memory) -> Result<Report, Error> {
    let structural = options
        .structural
        .as_deref()
        .map(|lang| Structural::new(&options.pattern, lang, None))
        .transpose()
        .map_err(|e| Error::Usage(e.to_string()))?;
    let matcher = match &structural {
        Some(_) => None,
        None => Some(
            RegexMatcherBuilder::new()
                .case_insensitive(options.ignore_case)
                .word(options.word)
                .fixed_strings(options.fixed_strings)
                .line_terminator(Some(b'\n'))
                .build(&options.pattern)
                .map_err(|e| Error::Usage(format!("invalid pattern: {e}")))?,
        ),
    };
    let structural = structural.as_ref();
    let filter = options.filter.as_deref();
    let paths = if options.paths.is_empty() {
        vec![PathBuf::from(".")]
    } else {
        options.paths.clone()
    };
    if let Some(missing) = paths.iter().find(|p| std::fs::metadata(p).is_err()) {
        return Err(Error::NotFound(missing.clone()));
    }
    let mut walk = ignore::WalkBuilder::new(&paths[0]);
    for path in &paths[1..] {
        walk.add(path);
    }
    walk.hidden(!options.hidden);
    if !options.globs.is_empty() {
        let mut overrides = OverrideBuilder::new(".");
        for glob in &options.globs {
            overrides
                .add(glob)
                .map_err(|e| Error::Usage(format!("invalid glob `{glob}`: {e}")))?;
        }
        walk.overrides(
            overrides
                .build()
                .map_err(|e| Error::Usage(format!("invalid glob: {e}")))?,
        );
    }
    // The most of one file that could ever be shown, so a file with a
    // million matches keeps only a screenful.
    let keep = match (options.budget.total, options.budget.per_file) {
        (0, 0) => usize::MAX,
        (0, n) | (n, 0) => n,
        (a, b) => a.min(b),
    };
    let memory_ref: &Memory = memory;
    let found: Mutex<Vec<Collected>> = Mutex::new(Vec::new());
    let searched = AtomicUsize::new(0);
    walk.build_parallel().run(|| {
        let matcher = matcher.clone();
        let mut searcher = SearcherBuilder::new()
            .binary_detection(BinaryDetection::quit(0))
            .line_number(true)
            .build();
        let found = &found;
        let searched = &searched;
        let note = Note {
            keep,
            memory: memory_ref,
        };
        Box::new(move |entry| {
            let Ok(entry) = entry else {
                return WalkState::Continue;
            };
            if !entry.file_type().is_some_and(|t| t.is_file())
                || structural.is_some_and(|s| !s.applies_to(entry.path()))
            {
                return WalkState::Continue;
            }
            searched.fetch_add(1, Ordering::Relaxed);
            let path = entry.path();
            let stamp = entry
                .metadata()
                .map_or(Stamp { len: 0, mtime: 0 }, |m| Stamp::of(&m));
            let mut key: Option<String> = None;
            let mut collected = Collected {
                path: path.to_path_buf(),
                key: String::new(),
                stamp,
                matches: 0,
                fresh: Vec::new(),
                already_shown: 0,
            };
            let mut kept = 0usize;
            let result = match (structural, &matcher) {
                (Some(structural), _) => std::fs::read_to_string(path).map(|source| {
                    let mut matches = structural.find(&source);
                    if let Some(filter) = filter {
                        matches = filter.keep(structural, path, &source, matches);
                    }
                    for m in matches {
                        note.add(&mut collected, &mut key, &mut kept, m.line, &m.text);
                    }
                }),
                (None, Some(matcher)) => searcher
                    .search_path(
                        matcher,
                        path,
                        Lossy(|line, text| {
                            let line = u32::try_from(line).unwrap_or(u32::MAX);
                            note.add(&mut collected, &mut key, &mut kept, line, text);
                            Ok(true)
                        }),
                    )
                    .map(|_| ()),
                (None, None) => Ok(()),
            };
            if result.is_ok() && collected.matches > 0 {
                collected.key = key.unwrap_or_default();
                if let Ok(mut found) = found.lock() {
                    found.push(collected);
                }
            }
            WalkState::Continue
        })
    });
    let mut found = found.into_inner().unwrap_or_default();
    found.sort_by(|a, b| a.path.cmp(&b.path));
    let mut report = Report {
        matches: found.iter().map(|c| c.matches).sum(),
        files_matched: found.len(),
        files_searched: searched.into_inner(),
        ..Report::default()
    };
    if options.files_only {
        list_files(&mut report, found, options.budget.total);
    } else {
        lay_out(&mut report, found, options, memory);
    }
    Ok(report)
}

/// How the walker keeps a file's matches: at most `keep` bytes of lines
/// (always the first), skipping lines the session was already shown.
#[derive(Clone, Copy)]
struct Note<'m> {
    keep: usize,
    memory: &'m Memory,
}

impl Note<'_> {
    fn add(
        &self,
        c: &mut Collected,
        key: &mut Option<String>,
        kept: &mut usize,
        line: u32,
        text: &str,
    ) {
        c.matches += 1;
        if self.memory.active() {
            let key = key.get_or_insert_with(|| memory::key(&c.path));
            if self.memory.shown(key, c.stamp, line) {
                c.already_shown += 1;
                return;
            }
        }
        if *kept < self.keep || c.fresh.is_empty() {
            let text = clip(text.trim());
            *kept += text.len() + 8;
            c.fresh.push((line, text));
        }
    }
}

/// Shows each file's matches, in order, until the budgets run out.
fn lay_out(report: &mut Report, found: Vec<Collected>, options: &Options, memory: &mut Memory) {
    let budget = options.budget;
    let fits = |used: usize, more: usize, limit: usize| limit == 0 || used + more <= limit;
    // The files the total budget can reach at all: their symbols are looked
    // up, in parallel, before the layout decides how much of each to show.
    let mut reach = 0usize;
    let mut least = 0usize;
    for c in &found {
        let cost = display(&c.path).len() + 1 + c.fresh.first().map_or(0, |(_, t)| t.len() + 10);
        if reach > 0 && !fits(least, cost, budget.total) {
            break;
        }
        least += cost;
        reach += 1;
    }
    let mut symbols = enclosing(&found[..reach], options.cache.as_deref()).into_iter();
    let mut used = 0usize;
    for c in found {
        let path = display(&c.path);
        let header = path.len() + 1;
        let first = c.fresh.first().map_or(0, |(_, t)| t.len() + 10);
        let file_symbols = symbols.next();
        if !report.files.is_empty() && !fits(used, header + first, budget.total) {
            report.omitted.push(Omitted {
                path,
                matches: c.matches,
            });
            continue;
        }
        used += header;
        let mut file = FileReport {
            path,
            matches: c.matches,
            already_shown: c.already_shown,
            ..FileReport::default()
        };
        let mut in_file = 0usize;
        let mut last: Option<Symbol> = None;
        let mut file_symbols = file_symbols.unwrap_or_default().into_iter();
        for (line, text) in c.fresh.iter() {
            let symbol = file_symbols.next().flatten();
            let mut cost = text.len() + 10;
            if symbol.is_some() && symbol != last {
                cost += symbol
                    .as_ref()
                    .map_or(0, |s| s.kind.len() + s.name.len() + 16);
            }
            let first_shown = report.files.is_empty() && file.shown.is_empty();
            if !first_shown
                && (!fits(in_file, cost, budget.per_file) || !fits(used, cost, budget.total))
            {
                break;
            }
            in_file += cost;
            used += cost;
            last.clone_from(&symbol);
            file.shown.push(Hit {
                line: *line,
                text: text.clone(),
                symbol,
            });
        }
        file.more = c.matches - c.already_shown - file.shown.len();
        if memory.active() {
            let key = if c.key.is_empty() {
                memory::key(&c.path)
            } else {
                c.key.clone()
            };
            for hit in &file.shown {
                memory.record(&key, c.stamp, hit.line, hit.line);
            }
        }
        report.files.push(file);
    }
}

/// The definition around each match the files may show, file by file, read
/// on as many threads as there are cores.
fn enclosing(files: &[Collected], cache: Option<&Path>) -> Vec<Vec<Option<Symbol>>> {
    let next = AtomicUsize::new(0);
    let done: Mutex<Vec<(usize, Vec<Option<Symbol>>)>> = Mutex::new(Vec::new());
    let threads = std::thread::available_parallelism()
        .map_or(4, |n| n.get())
        .min(files.len());
    std::thread::scope(|scope| {
        for _ in 0..threads {
            scope.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(c) = files.get(i) else {
                        break;
                    };
                    let lines: Vec<u32> = c.fresh.iter().map(|(line, _)| *line).collect();
                    let defs = match cache {
                        Some(dir) => cached_defs(dir, &c.path, c.stamp),
                        None => std::fs::read(&c.path)
                            .ok()
                            .and_then(|source| symbols::definitions(&c.path, &source)),
                    };
                    let defs = defs.unwrap_or_default();
                    let defs: Vec<Option<symbols::Def>> = lines
                        .iter()
                        .map(|l| symbols::enclosing(&defs, *l).cloned())
                        .collect();
                    let found = defs
                        .into_iter()
                        .map(|d| {
                            d.map(|d| Symbol {
                                kind: d.kind,
                                name: d.name,
                                start: d.start,
                                end: d.end,
                            })
                        })
                        .collect();
                    if let Ok(mut done) = done.lock() {
                        done.push((i, found));
                    }
                }
            });
        }
    });
    let mut done = done.into_inner().unwrap_or_default();
    done.sort_by_key(|(i, _)| *i);
    done.into_iter().map(|(_, symbols)| symbols).collect()
}

#[derive(Serialize, serde::Deserialize)]
struct CachedDefs {
    stamp: Stamp,
    queries: u64,
    defs: Vec<symbols::Def>,
}

/// A file's definitions, from the cache when the file is unchanged since
/// they were stored, else parsed and stored. A large file takes tens of
/// milliseconds to parse, and an agent searches the same files again and
/// again.
fn cached_defs(dir: &Path, path: &Path, stamp: Stamp) -> Option<Vec<symbols::Def>> {
    if !symbols::supported(path) {
        return None;
    }
    let key = memory::key(path);
    let file = dir.join(format!(
        "{:016x}.json",
        symbols::fnv(0xcbf2_9ce4_8422_2325, key.as_bytes())
    ));
    if let Some(cached) = std::fs::read(&file)
        .ok()
        .and_then(|b| serde_json::from_slice::<CachedDefs>(&b).ok())
        .filter(|c| c.stamp == stamp && c.queries == symbols::queries_hash())
    {
        return Some(cached.defs);
    }
    let defs = symbols::definitions(path, &std::fs::read(path).ok()?)?;
    let doc = CachedDefs {
        stamp,
        queries: symbols::queries_hash(),
        defs,
    };
    prune(dir);
    let _ = std::fs::create_dir_all(dir).and_then(|()| {
        let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
        serde_json::to_writer(&mut tmp, &doc)?;
        tmp.persist(&file).map_err(|e| e.error)?;
        Ok(())
    });
    Some(doc.defs)
}

/// Removes cached definitions untouched for a month, at most once a day.
fn prune(dir: &Path) {
    const DAY: std::time::Duration = std::time::Duration::from_secs(24 * 60 * 60);
    let marker = dir.join(".pruned");
    let now = std::time::SystemTime::now();
    let age = |p: &Path| {
        std::fs::metadata(p)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| now.duration_since(t).ok())
    };
    if age(&marker).is_some_and(|a| a < DAY) {
        return;
    }
    let _ = std::fs::create_dir_all(dir).and_then(|()| std::fs::write(&marker, b""));
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "json") && age(&path).is_some_and(|a| a > 30 * DAY)
        {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// `--files-with-matches`: one line per file, until the budget runs out.
fn list_files(report: &mut Report, found: Vec<Collected>, total: usize) {
    let mut used = 0usize;
    for c in found {
        let path = display(&c.path);
        let cost = path.len() + 8;
        if total != 0 && !report.files.is_empty() && used + cost > total {
            report.omitted.push(Omitted {
                path,
                matches: c.matches,
            });
            continue;
        }
        used += cost;
        report.files.push(FileReport {
            path,
            matches: c.matches,
            ..FileReport::default()
        });
    }
}

/// A path as the agent typed it, without a leading `./`.
fn display(path: &Path) -> String {
    let path = path.strip_prefix(".").unwrap_or(path);
    path.to_string_lossy().into_owned()
}

/// A matching line cut to [`LINE_MAX`] bytes on a character boundary, with
/// what was cut said.
fn clip(text: &str) -> String {
    if text.len() <= LINE_MAX {
        return text.to_string();
    }
    let mut cut = LINE_MAX;
    while !text.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{} ... (+{} bytes)", &text[..cut], text.len() - cut)
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// The report as text: each file, its matches under their enclosing
/// definitions, and a marker for everything left out.
pub fn render(report: &Report, files_only: bool) -> String {
    let mut text = String::new();
    if report.files.is_empty() {
        let _ = writeln!(
            text,
            "no matches ({} searched)",
            plural(report.files_searched, "file", "files")
        );
        return text;
    }
    for file in &report.files {
        if files_only {
            let _ = writeln!(text, "{} ({})", file.path, file.matches);
            continue;
        }
        if file.shown.is_empty() && file.more == 0 {
            let _ = writeln!(
                text,
                "{}: {} already shown, unchanged",
                file.path,
                plural(file.already_shown, "match", "matches")
            );
            continue;
        }
        let _ = writeln!(text, "{}", file.path);
        let mut last: Option<&Symbol> = None;
        for hit in &file.shown {
            match &hit.symbol {
                Some(symbol) => {
                    if last != Some(symbol) {
                        let _ = writeln!(
                            text,
                            "  {} {} ({}-{})",
                            symbol.kind, symbol.name, symbol.start, symbol.end
                        );
                    }
                    let _ = writeln!(text, "    {}: {}", hit.line, hit.text);
                }
                None => {
                    let _ = writeln!(text, "  {}: {}", hit.line, hit.text);
                }
            }
            last = hit.symbol.as_ref();
        }
        if file.more > 0 {
            let _ = writeln!(text, "  ... {} more in {}", file.more, file.path);
        }
        if file.already_shown > 0 {
            let _ = writeln!(
                text,
                "  ... {} already shown, unchanged",
                file.already_shown
            );
        }
    }
    if !report.omitted.is_empty() {
        let matches: usize = report.omitted.iter().map(|o| o.matches).sum();
        let named: Vec<String> = report
            .omitted
            .iter()
            .take(OMITTED_NAMED)
            .map(|o| format!("{} ({})", o.path, o.matches))
            .collect();
        let rest = report.omitted.len().saturating_sub(OMITTED_NAMED);
        let _ = write!(
            text,
            "... {} in {} more {}: {}",
            plural(matches, "match", "matches"),
            report.omitted.len(),
            if report.omitted.len() == 1 {
                "file"
            } else {
                "files"
            },
            named.join(", ")
        );
        if rest > 0 {
            let _ = write!(text, " and {}", plural(rest, "other", "others"));
        }
        text.push_str(" (narrow the search, or raise --budget)\n");
    }
    text
}

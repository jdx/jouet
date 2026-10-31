//! `jouet docs`: a dependency's public API at the version the project
//! locked, read from the sources already installed for it.
//!
//! [`locate`] finds the package in the nearest lockfile (`Cargo.lock`, an
//! npm-family lockfile, `uv.lock` or `poetry.lock`, `go.mod`) and its
//! installed sources at that version (the cargo registry, `node_modules`, the
//! virtualenv's site-packages, the Go module cache). It never downloads: a
//! package that is locked but not installed is reported with the command
//! that installs it.
//!
//! [`index`] runs the code graph's extractor over those sources, decides
//! what is public by each language's rules, and caches the result under
//! jouet's cache directory, keyed by name and version, so a second lookup
//! reads one file.

mod golang;
mod js;
mod locate;
mod python;
mod rust;
mod space;
mod text;

pub(crate) use locate::{Lang, Missing, Package, locate};
pub(crate) use text::summary;

use jouet_graph::extract::Node;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// Bumped when the index changes shape or meaning, so a cached one is
/// rebuilt.
const FORMAT: u32 = 1;

/// A source file larger than this is left out of the index: generated code.
const MAX_FILE_BYTES: u64 = 2 << 20;

/// The most files one package's index reads.
const MAX_FILES: usize = 4000;

/// One definition of the package.
#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Item {
    pub(crate) kind: String,
    pub(crate) name: String,
    /// Where the package exposes it, the shortest first: `ureq::Agent`,
    /// `click.Context.invoke`. A private definition has its own path.
    pub(crate) paths: Vec<String>,
    pub(crate) public: bool,
    /// Relative to the package's root, with `/` separators.
    pub(crate) file: String,
    pub(crate) line: u32,
    pub(crate) end_line: u32,
    /// The declaration without its body.
    pub(crate) signature: String,
    pub(crate) doc: Option<String>,
    /// For a method, field or variant: the definition it belongs to.
    pub(crate) owner: Option<usize>,
    /// Where the definition is when no public path reaches it.
    #[serde(skip)]
    home: String,
}

impl Item {
    fn new(kind: &str, name: &str, file: &str, node_lines: (u32, u32)) -> Item {
        Item {
            kind: kind.to_string(),
            name: name.to_string(),
            paths: Vec::new(),
            public: false,
            file: file.to_string(),
            line: node_lines.0,
            end_line: node_lines.1,
            signature: String::new(),
            doc: None,
            owner: None,
            home: String::new(),
        }
    }
}

/// A package's definitions.
#[derive(Serialize, Deserialize)]
pub(crate) struct Index {
    format: u32,
    /// The package's own doc: a crate's `//!`, a module docstring.
    pub(crate) doc: Option<String>,
    pub(crate) items: Vec<Item>,
}

/// The package's index, from the cache or built now.
pub(crate) fn index(pkg: &Package) -> Result<Index, String> {
    let cache = cache_path(pkg);
    if let Some(index) = cache
        .as_ref()
        .and_then(|path| std::fs::read(path).ok())
        .and_then(|bytes| serde_json::from_slice::<Index>(&bytes).ok())
        .filter(|index| index.format == FORMAT)
    {
        return Ok(index);
    }
    let mut index = match pkg.lang {
        Lang::Rust => rust::build(pkg)?,
        Lang::Ts => js::build(pkg)?,
        Lang::Python => python::build(pkg)?,
        Lang::Go => golang::build(pkg)?,
    };
    index.format = FORMAT;
    if let Some(path) = cache {
        save(&path, &index);
    }
    Ok(index)
}

/// `<cache>/docs/<lang>/<name>@<version>-<hash of where it is installed>.json`:
/// the same version installed in two places (two `node_modules`) may differ.
fn cache_path(pkg: &Package) -> Option<PathBuf> {
    let dir = jouet_output::paths::cache_dir()?
        .join("docs")
        .join(pkg.lang.key());
    let digest = Sha256::digest(pkg.api_root.to_string_lossy().as_bytes());
    let hash: String = digest.iter().take(6).map(|b| format!("{b:02x}")).collect();
    let name: String = pkg
        .name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "-_.".contains(c) {
                c
            } else {
                '+'
            }
        })
        .collect();
    Some(dir.join(format!("{name}@{}-{hash}.json", pkg.version)))
}

/// Writes the cache through a temporary file, so a reader never sees half
/// of one. A cache that cannot be written is only slower next time.
fn save(path: &Path, index: &Index) {
    let Some(dir) = path.parent() else { return };
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let Ok(bytes) = serde_json::to_vec(index) else {
        return;
    };
    if let Ok(mut tmp) = tempfile::NamedTempFile::new_in(dir) {
        use std::io::Write as _;
        if tmp.write_all(&bytes).is_ok() {
            let _ = tmp.persist(path);
        }
    }
}

/// A source file and the definitions the extractor found in it.
pub(super) struct Source {
    /// Relative to the package's root, with `/` separators.
    pub(super) rel: String,
    pub(super) text: String,
    pub(super) nodes: Vec<Node>,
}

impl Source {
    pub(super) fn lines(&self) -> Vec<&str> {
        self.text.lines().collect()
    }
}

/// Reads and extracts `rels` under `root`, on as many threads as there are
/// cores. A file that cannot be read or parsed has no definitions.
pub(super) fn parse(root: &Path, rels: Vec<String>) -> Vec<Source> {
    let threads = std::thread::available_parallelism()
        .map_or(4, std::num::NonZero::get)
        .clamp(1, 16);
    let chunk = rels.len().div_ceil(threads).max(1);
    std::thread::scope(|scope| {
        let workers: Vec<_> = rels
            .chunks(chunk)
            .map(|part| {
                scope.spawn(move || {
                    part.iter()
                        .filter_map(|rel| {
                            let path = root.join(rel);
                            let meta = std::fs::metadata(&path).ok()?;
                            if meta.len() > MAX_FILE_BYTES {
                                return None;
                            }
                            let text = String::from_utf8(std::fs::read(&path).ok()?).ok()?;
                            // A stub is Python to the extractor.
                            let as_path = rel.strip_suffix(".pyi").map(|s| format!("{s}.py"));
                            let nodes = match jouet_graph::extract::extract(
                                as_path.as_deref().unwrap_or(rel),
                                &text,
                            ) {
                                Some(Ok(extraction)) => extraction.nodes,
                                _ => Vec::new(),
                            };
                            Some(Source {
                                rel: rel.clone(),
                                text,
                                nodes,
                            })
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        workers
            .into_iter()
            .flat_map(|w| w.join().unwrap_or_default())
            .collect()
    })
}

/// The files under `root` that `keep` accepts, relative and sorted, in
/// directories `enter` accepts, at most [`MAX_FILES`].
pub(super) fn walk(
    root: &Path,
    keep: impl Fn(&str) -> bool,
    enter: impl Fn(&str) -> bool,
) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![String::new()];
    while let Some(rel) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(root.join(&rel)) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let path = if rel.is_empty() {
                name.clone()
            } else {
                format!("{rel}/{name}")
            };
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                if !name.starts_with('.') && enter(&name) {
                    stack.push(path);
                }
            } else if keep(&name) && out.len() < MAX_FILES {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

/// The segments of a path or query: `a::b::C`, `a.b.C`.
pub(crate) fn segments(text: &str) -> Vec<&str> {
    text.split("::")
        .flat_map(|part| part.split('.'))
        .filter(|s| !s.is_empty())
        .collect()
}

/// The definitions `query` names: those with a path equal to it (or, where
/// paths start with the package's root, `rooted`, equal to it after the
/// root: `spawn` is `tokio::spawn`), else those with a path ending in it.
/// The shortest paths come first.
pub(crate) fn find(index: &Index, query: &str, private: bool, rooted: bool) -> Vec<usize> {
    let wanted = segments(query);
    if wanted.is_empty() {
        return Vec::new();
    }
    let visible = |item: &Item| private || item.public;
    let matching = |exact: bool| -> Vec<usize> {
        index
            .items
            .iter()
            .enumerate()
            .filter(|(_, item)| visible(item))
            .filter(|(_, item)| {
                item.paths.iter().any(|p| {
                    let have = segments(p);
                    if exact {
                        have == wanted || rooted && have.get(1..) == Some(wanted.as_slice())
                    } else {
                        have.ends_with(&wanted)
                    }
                })
            })
            .map(|(i, _)| i)
            .collect()
    };
    let exact = matching(true);
    let mut found = if exact.is_empty() {
        matching(false)
    } else {
        exact
    };
    let shortest = |i: &usize| {
        let path = index.items[*i].paths.first().cloned().unwrap_or_default();
        (segments(&path).len(), path)
    };
    found.sort_by_cached_key(shortest);
    found
}

/// Names like `query`'s last segment, for a lookup that found nothing.
pub(crate) fn near(index: &Index, query: &str, private: bool, max: usize) -> Vec<String> {
    let last = segments(query)
        .last()
        .map(|s| s.to_lowercase())
        .unwrap_or_default();
    let mut names: Vec<String> = index
        .items
        .iter()
        .filter(|item| (private || item.public) && item.name.to_lowercase().contains(&last))
        .filter_map(|item| item.paths.first().cloned())
        .collect();
    names.sort_by_key(|n| (segments(n).len(), n.len()));
    names.dedup();
    names.truncate(max);
    names
}

/// The members of `owner`: its methods, fields and variants.
pub(crate) fn members(index: &Index, owner: usize, private: bool) -> Vec<usize> {
    index
        .items
        .iter()
        .enumerate()
        .filter(|(_, item)| item.owner == Some(owner) && (private || item.public))
        .map(|(i, _)| i)
        .collect()
}

/// The top-level definitions an outline lists, in the order it lists them:
/// the package's root first, then deeper modules; definitions before
/// re-exports of other packages, and modules last.
pub(crate) fn outline(index: &Index, private: bool, within: Option<&str>) -> Vec<usize> {
    let within: Vec<&str> = within.map(segments).unwrap_or_default();
    let mut top: Vec<usize> = index
        .items
        .iter()
        .enumerate()
        .filter(|(_, item)| item.owner.is_none() && (private || item.public))
        // A package's root module is the package itself.
        .filter(|(_, item)| {
            item.kind != "module" || item.paths.first().is_none_or(|p| segments(p).len() > 1)
        })
        .filter(|(_, item)| {
            within.is_empty()
                || item.paths.iter().any(|p| {
                    let have = segments(p);
                    have.len() > within.len() && have[..have.len() - 1].ends_with(&within)
                })
        })
        .map(|(i, _)| i)
        .collect();
    let rank = |kind: &str| match kind {
        "module" => 2,
        "reexport" => 1,
        _ => 0,
    };
    top.sort_by(|&a, &b| {
        let (a, b) = (&index.items[a], &index.items[b]);
        let path = |item: &Item| item.paths.first().cloned().unwrap_or_default();
        (segments(&path(a)).len(), rank(&a.kind), path(a)).cmp(&(
            segments(&path(b)).len(),
            rank(&b.kind),
            path(b),
        ))
    });
    top
}

/// Fills each item's `public` and, for one no public path reaches, its own
/// path; gives members their owners' paths.
fn finish(items: &mut [Item], sep: &str) {
    for i in 0..items.len() {
        let Some(owner) = items[i].owner else {
            continue;
        };
        let name = items[i].name.clone();
        let owner_paths = items[owner].paths.clone();
        let owner_public = !owner_paths.is_empty();
        items[i].public &= owner_public;
        if items[i].public {
            items[i].paths = owner_paths
                .iter()
                .map(|p| format!("{p}{sep}{name}"))
                .collect();
        }
    }
    for item in items.iter_mut() {
        if item.owner.is_none() {
            item.public = !item.paths.is_empty();
        }
        if !item.public || item.paths.is_empty() {
            item.paths = vec![std::mem::take(&mut item.home)];
        }
    }
}

/// Paths the shortest first, without duplicates.
fn sort_paths(items: &mut [Item]) {
    for item in items {
        item.paths.sort_by_key(|p| (segments(p).len(), p.len()));
        item.paths.dedup();
    }
}

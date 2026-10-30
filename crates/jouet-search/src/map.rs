//! `jouet read --map`: a map of the repository's most important definitions
//! that fits a token budget.
//!
//! The design and its weights are Aider's repo map (Apache-2.0,
//! <https://github.com/Aider-AI/aider/blob/main/aider/repomap.py>), rebuilt
//! in Rust:
//!
//! - every source file's definitions and references come from tree-sitter's
//!   tags queries, cached by modification time;
//! - a file that refers to a name gets an edge to each file defining it,
//!   weighted ×10 for a focused identifier or a long distinctive one
//!   (snake, kebab or camel case, 8 characters or more), ×0.1 for a name
//!   starting with `_` or defined in more than 5 files, ×50 when the
//!   referring file is focused, and by the square root of the number of
//!   references;
//! - personalised PageRank over that graph ranks the files, and each file's
//!   rank is shared among the definitions its edges point at;
//! - a binary search finds how many of the ranked definitions fit the
//!   budget.
//!
//! It departs from Aider in three places. A reference to a name defined in
//! several files is shared among them rather than counted in full for
//! each, which keeps a common method name such as `is_empty` from crowding
//! out a repository's own types. Rust also gets the type references its
//! grammar's query leaves out (see [`crate::symbols`]). And the definitions
//! of a focused identifier are listed first.

use crate::symbols::{self, Def, FileSymbols};
use ignore::WalkState;
use petgraph::Direction;
use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::visit::EdgeRef;
use serde::{Deserialize, Serialize};
use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Aider's default budget, with focus files.
pub const DEFAULT_BUDGET: usize = 1024;
/// Without focus files the map is the agent's whole view of the
/// repository, so Aider multiplies the budget by this.
pub const UNFOCUSED_MULTIPLIER: usize = 8;
/// Files larger than this are skipped: they are generated or vendored far
/// more often than written.
const MAX_FILE: u64 = 1024 * 1024;
/// Bumped whenever the cached shape changes; a change to a tags query
/// changes [`symbols::queries_hash`] instead.
const CACHE_VERSION: u64 = 2;

/// What to map.
#[derive(Debug, Default)]
pub struct Options {
    /// The directory to map; paths in the map are relative to it.
    pub root: PathBuf,
    /// Files the agent is working on, relative to `root`. Their own
    /// definitions are left out, as they are already in view.
    pub focus_files: Vec<String>,
    /// Identifiers the agent is interested in.
    pub focus_idents: Vec<String>,
    /// The budget in tokens (estimated at 4 bytes each).
    pub budget: usize,
    /// Where to cache tags; nothing is cached when `None`.
    pub cache: Option<PathBuf>,
}

/// A map, ready to print.
#[derive(Debug, Default, Serialize)]
pub struct RepoMap {
    pub files: Vec<MapFile>,
    /// The map's estimated size in tokens.
    pub tokens: usize,
    /// Source files read for the map.
    pub files_scanned: usize,
    /// Of those, the ones whose tags came from the cache.
    #[serde(skip_serializing_if = "is_zero")]
    pub cached: usize,
    /// Ranked definitions and files that did not fit the budget.
    #[serde(skip_serializing_if = "is_zero")]
    pub more_defs: usize,
    #[serde(skip_serializing_if = "is_zero")]
    pub more_files: usize,
}

#[derive(Debug, Serialize)]
pub struct MapFile {
    pub path: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub defs: Vec<MapDef>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MapDef {
    pub line: u32,
    pub kind: String,
    pub name: String,
    pub text: String,
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

/// A file's symbols as the map uses them: its definitions, and how many
/// times it refers to each name.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Tags {
    defs: Vec<Def>,
    refs: Vec<(String, u32)>,
}

impl From<FileSymbols> for Tags {
    fn from(symbols: FileSymbols) -> Self {
        let mut counts: BTreeMap<String, u32> = BTreeMap::new();
        for r in symbols.refs {
            match counts.entry(r.name) {
                Entry::Occupied(mut e) => *e.get_mut() += 1,
                Entry::Vacant(e) => {
                    e.insert(1);
                }
            }
        }
        Self {
            defs: symbols.defs,
            refs: counts.into_iter().collect(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Cached {
    len: u64,
    mtime: u128,
    tags: Tags,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct CacheFile {
    version: u64,
    queries: u64,
    files: HashMap<String, Cached>,
}

/// Builds the map.
pub fn build(options: &Options) -> RepoMap {
    let (tags, cached) = scan(&options.root, options.cache.as_deref());
    let files_scanned = tags.len();
    let ranked = rank(&tags, options);
    let mut map = fit(&ranked, &tags, options.budget);
    map.files_scanned = files_scanned;
    map.cached = cached;
    map
}

/// Every source file under `root` with its symbols, reusing the cache for
/// files whose size and modification time are unchanged. Returns how many
/// came from the cache too.
fn scan(root: &Path, cache: Option<&Path>) -> (BTreeMap<String, Tags>, usize) {
    let cache_file = cache.map(|dir| dir.join(cache_name(root)));
    let old: CacheFile = cache_file
        .as_deref()
        .and_then(|f| std::fs::read(f).ok())
        .and_then(|b| serde_json::from_slice::<CacheFile>(&b).ok())
        .filter(|c| c.version == CACHE_VERSION && c.queries == symbols::queries_hash())
        .unwrap_or_default();
    let files: Mutex<Vec<(String, PathBuf, u64, u128)>> = Mutex::new(Vec::new());
    ignore::WalkBuilder::new(root).build_parallel().run(|| {
        let files = &files;
        Box::new(move |entry| {
            let Ok(entry) = entry else {
                return WalkState::Continue;
            };
            if !entry.file_type().is_some_and(|t| t.is_file()) || !symbols::supported(entry.path())
            {
                return WalkState::Continue;
            }
            let Ok(meta) = entry.metadata() else {
                return WalkState::Continue;
            };
            if meta.len() > MAX_FILE {
                return WalkState::Continue;
            }
            let stamp = crate::memory::Stamp::of(&meta);
            let rel = relative(root, entry.path());
            if let Ok(mut files) = files.lock() {
                files.push((rel, entry.path().to_path_buf(), stamp.len, stamp.mtime));
            }
            WalkState::Continue
        })
    });
    let files = files.into_inner().unwrap_or_default();
    let next = AtomicUsize::new(0);
    let reused = AtomicUsize::new(0);
    let results: Mutex<Vec<(String, Cached)>> = Mutex::new(Vec::with_capacity(files.len()));
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    std::thread::scope(|scope| {
        for _ in 0..threads {
            scope.spawn(|| {
                let mut local = Vec::new();
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some((rel, path, len, mtime)) = files.get(i) else {
                        break;
                    };
                    if let Some(hit) = old
                        .files
                        .get(rel)
                        .filter(|c| c.len == *len && c.mtime == *mtime)
                    {
                        reused.fetch_add(1, Ordering::Relaxed);
                        local.push((rel.clone(), hit.clone()));
                        continue;
                    }
                    let tags = std::fs::read(path)
                        .ok()
                        .and_then(|source| symbols::parse(path, &source))
                        .unwrap_or_default()
                        .into();
                    local.push((
                        rel.clone(),
                        Cached {
                            len: *len,
                            mtime: *mtime,
                            tags,
                        },
                    ));
                }
                if let Ok(mut results) = results.lock() {
                    results.extend(local);
                }
            });
        }
    });
    let results = results.into_inner().unwrap_or_default();
    let reused = reused.into_inner();
    if let Some(file) =
        cache_file.filter(|_| reused != results.len() || old.files.len() != results.len())
    {
        let doc = CacheFile {
            version: CACHE_VERSION,
            queries: symbols::queries_hash(),
            files: results.iter().cloned().collect(),
        };
        let _ = write_cache(&file, &doc);
    }
    let tags = results.into_iter().map(|(rel, c)| (rel, c.tags)).collect();
    (tags, reused)
}

fn write_cache(file: &Path, doc: &CacheFile) -> std::io::Result<()> {
    let dir = file.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    serde_json::to_writer(std::io::BufWriter::new(&mut tmp), doc)?;
    tmp.persist(file).map_err(|e| e.error)?;
    Ok(())
}

/// The cache file for a root: a hash of its canonical path, so two
/// checkouts never share one.
fn cache_name(root: &Path) -> String {
    let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let hash = symbols::fnv(0xcbf2_9ce4_8422_2325, root.to_string_lossy().as_bytes());
    format!("{hash:016x}.json")
}

/// `path` relative to `root`, with `/` separators.
fn relative(root: &Path, path: &Path) -> String {
    let rel = path.strip_prefix(root).unwrap_or(path);
    rel.components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// An entry of the ranked list: a definition, or a file with nothing else
/// to show.
#[derive(Debug, Clone, PartialEq)]
enum Ranked {
    Def(String, usize),
    File(String),
}

/// Ranks every definition, most important first, following Aider's
/// `get_ranked_tags`.
fn rank(tags: &BTreeMap<String, Tags>, options: &Options) -> Vec<Ranked> {
    // Files are numbered in path order, so the graph never hashes a path.
    let files: Vec<&str> = tags.keys().map(String::as_str).collect();
    let focus_idents: HashSet<&str> = options.focus_idents.iter().map(String::as_str).collect();
    let focused: Vec<bool> = files
        .iter()
        .map(|f| options.focus_files.iter().any(|x| x == f))
        .collect();
    let mut defines: HashMap<&str, Vec<usize>> = HashMap::new();
    let mut references: HashMap<&str, Vec<(usize, u32)>> = HashMap::new();
    let mut definitions: HashMap<(usize, &str), Vec<usize>> = HashMap::new();
    let mut personalization = vec![0.0; files.len()];
    let personalize = 100.0 / files.len().max(1) as f64;
    for (id, file_tags) in tags.values().enumerate() {
        if focused[id] {
            personalization[id] += personalize;
        }
        // A file whose path names a focused identifier counts too.
        let path = Path::new(files[id]);
        let mut parts: HashSet<&str> = path.iter().filter_map(|p| p.to_str()).collect();
        parts.extend(path.file_stem().and_then(|s| s.to_str()));
        if !focus_idents.is_empty() && parts.iter().any(|p| focus_idents.contains(p)) {
            personalization[id] += personalize;
        }
        for (i, def) in file_tags.defs.iter().enumerate() {
            let definers = defines.entry(&def.name).or_default();
            if definers.last() != Some(&id) {
                definers.push(id);
            }
            definitions.entry((id, &def.name)).or_default().push(i);
        }
        for (name, count) in &file_tags.refs {
            references.entry(name).or_default().push((id, *count));
        }
    }
    if references.is_empty() {
        references = defines
            .iter()
            .map(|(name, ids)| (*name, ids.iter().map(|id| (*id, 1)).collect()))
            .collect();
    }

    let mut graph: DiGraph<usize, (f64, &str)> = DiGraph::new();
    let mut nodes: Vec<Option<NodeIndex>> = vec![None; files.len()];
    let mut node = |graph: &mut DiGraph<usize, (f64, &str)>, id: usize| {
        *nodes[id].get_or_insert_with(|| graph.add_node(id))
    };
    let mut idents: Vec<&str> = defines.keys().copied().collect();
    idents.sort_unstable();
    for ident in &idents {
        let definers = &defines[ident];
        let Some(referrers) = references.get(ident) else {
            // A definition nothing refers to still counts a little.
            for &definer in definers {
                let n = node(&mut graph, definer);
                graph.add_edge(n, n, (0.1, ident));
            }
            continue;
        };
        let mut mul = 1.0;
        let has_alpha = ident.chars().any(char::is_alphabetic);
        let snake = ident.contains('_') && has_alpha;
        let kebab = ident.contains('-') && has_alpha;
        let camel = ident.chars().any(char::is_uppercase) && ident.chars().any(char::is_lowercase);
        if focus_idents.contains(ident) {
            mul *= 10.0;
        }
        if (snake || kebab || camel) && ident.chars().count() >= 8 {
            mul *= 10.0;
        }
        if ident.starts_with('_') {
            mul *= 0.1;
        }
        if definers.len() > 5 {
            mul *= 0.1;
        }
        // A reference to a name defined in several files is evidence for
        // each of them only in part.
        let share = 1.0 / definers.len() as f64;
        for &(referrer, count) in referrers {
            let mut weight = mul * share * f64::from(count).sqrt();
            if focused[referrer] {
                weight *= 50.0;
            }
            let from = node(&mut graph, referrer);
            for &definer in definers {
                let to = node(&mut graph, definer);
                graph.add_edge(from, to, (weight, ident));
            }
        }
    }

    let personal: Vec<f64> = graph
        .node_indices()
        .map(|n| personalization[graph[n]])
        .collect();
    let ranks = pagerank(&graph, &personal);

    // Share each file's rank among the definitions its edges point at.
    let mut def_rank: HashMap<(usize, &str), f64> = HashMap::new();
    for n in graph.node_indices() {
        let total: f64 = graph
            .edges_directed(n, Direction::Outgoing)
            .map(|e| e.weight().0)
            .sum();
        if total <= 0.0 {
            continue;
        }
        for e in graph.edges_directed(n, Direction::Outgoing) {
            let (weight, ident) = *e.weight();
            *def_rank.entry((graph[e.target()], ident)).or_default() +=
                ranks[n.index()] * weight / total;
        }
    }
    let mut ordered: Vec<((usize, &str), f64)> = def_rank.into_iter().collect();
    ordered.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| b.0.cmp(&a.0)));
    let mut ranked = Vec::new();
    let mut included = vec![false; files.len()];
    for ((file, ident), _) in ordered {
        if focused[file] {
            continue;
        }
        for &i in definitions.get(&(file, ident)).into_iter().flatten() {
            ranked.push(Ranked::Def(files[file].to_string(), i));
        }
        included[file] = true;
    }
    // Definitions of a focused identifier come first: the agent named it.
    if !focus_idents.is_empty() {
        let named = |r: &Ranked| match r {
            Ranked::Def(file, i) => tags
                .get(file)
                .and_then(|s| s.defs.get(*i))
                .is_some_and(|d| focus_idents.contains(d.name.as_str())),
            Ranked::File(_) => false,
        };
        let (first, rest): (Vec<Ranked>, Vec<Ranked>) = ranked.into_iter().partition(named);
        ranked = first.into_iter().chain(rest).collect();
    }
    // Then files by their own rank, and the files with no edges at all.
    let mut by_rank: Vec<(f64, usize)> = graph
        .node_indices()
        .map(|n| (ranks[n.index()], graph[n]))
        .collect();
    by_rank.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| b.1.cmp(&a.1)));
    let mut listed = vec![false; files.len()];
    let order = by_rank.into_iter().map(|(_, id)| id).chain(0..files.len());
    for id in order {
        if !included[id] && !focused[id] && !listed[id] {
            listed[id] = true;
            ranked.push(Ranked::File(files[id].to_string()));
        }
    }
    ranked
}

/// Personalised PageRank, as networkx computes it: damping 0.85, at most
/// 100 iterations, and dangling files' rank spread by the personalisation
/// (or evenly without one).
fn pagerank<N>(graph: &DiGraph<N, (f64, &str)>, personal: &[f64]) -> Vec<f64> {
    const ALPHA: f64 = 0.85;
    const TOLERANCE: f64 = 1.0e-6;
    let n = graph.node_count();
    if n == 0 {
        return Vec::new();
    }
    let total: f64 = personal.iter().sum();
    let p: Vec<f64> = if total > 0.0 {
        personal.iter().map(|v| v / total).collect()
    } else {
        vec![1.0 / n as f64; n]
    };
    let out_weight: Vec<f64> = graph
        .node_indices()
        .map(|i| {
            graph
                .edges_directed(i, Direction::Outgoing)
                .map(|e| e.weight().0)
                .sum()
        })
        .collect();
    let mut x = vec![1.0 / n as f64; n];
    for _ in 0..100 {
        let last = x.clone();
        x.iter_mut().for_each(|v| *v = 0.0);
        let dangling: f64 = ALPHA
            * graph
                .node_indices()
                .filter(|i| out_weight[i.index()] <= 0.0)
                .map(|i| last[i.index()])
                .sum::<f64>();
        for e in graph.edge_references() {
            let from = e.source().index();
            x[e.target().index()] += ALPHA * last[from] * e.weight().0 / out_weight[from];
        }
        for i in 0..n {
            x[i] += dangling * p[i] + (1.0 - ALPHA) * p[i];
        }
        let err: f64 = x.iter().zip(&last).map(|(a, b)| (a - b).abs()).sum();
        if err < n as f64 * TOLERANCE {
            break;
        }
    }
    x
}

/// The largest prefix of the ranked list whose rendering fits the budget,
/// found by binary search, as Aider does.
fn fit(ranked: &[Ranked], tags: &BTreeMap<String, Tags>, budget: usize) -> RepoMap {
    let mut best = (0, assemble(&[], tags));
    let (mut low, mut high) = (0, ranked.len());
    // Aider's first guess: about 25 tokens a definition.
    let mut middle = (budget / 25).min(ranked.len());
    while low <= high {
        let map = assemble(&ranked[..middle], tags);
        if map.tokens <= budget {
            if middle >= best.0 {
                best = (middle, map);
            }
            // Within 15% of the budget is close enough.
            if (budget - best.1.tokens) * 100 < budget * 15 {
                break;
            }
            low = middle + 1;
        } else if middle == 0 {
            break;
        } else {
            high = middle - 1;
        }
        middle = (low + high) / 2;
    }
    let (shown, mut map) = best;
    let rest = &ranked[shown..];
    map.more_defs = rest.iter().filter(|r| matches!(r, Ranked::Def(..))).count();
    let in_map: HashSet<&str> = map.files.iter().map(|f| f.path.as_str()).collect();
    let missing: HashSet<&str> = rest
        .iter()
        .map(|(Ranked::Def(file, _) | Ranked::File(file))| file.as_str())
        .filter(|file| !in_map.contains(file))
        .collect();
    map.more_files = missing.len();
    map
}

/// The map for a prefix of the ranked list: its files in path order, each
/// with its definitions in line order.
fn assemble(ranked: &[Ranked], tags: &BTreeMap<String, Tags>) -> RepoMap {
    let mut files: BTreeMap<&str, BTreeMap<u32, &Def>> = BTreeMap::new();
    for r in ranked {
        match r {
            Ranked::Def(file, i) => {
                if let Some(def) = tags.get(file).and_then(|s| s.defs.get(*i)) {
                    files.entry(file).or_default().insert(def.line, def);
                }
            }
            Ranked::File(file) => {
                files.entry(file).or_default();
            }
        }
    }
    let mut map = RepoMap {
        files: files
            .into_iter()
            .map(|(path, defs)| MapFile {
                path: path.to_string(),
                defs: defs
                    .into_values()
                    .map(|d| MapDef {
                        line: d.line,
                        kind: d.kind.clone(),
                        name: d.name.clone(),
                        text: d.text.clone(),
                    })
                    .collect(),
            })
            .collect(),
        ..RepoMap::default()
    };
    map.tokens = render_files(&map.files).len().div_ceil(4);
    map
}

fn render_files(files: &[MapFile]) -> String {
    let mut text = String::new();
    for file in files {
        let _ = writeln!(text, "{}", file.path);
        for def in &file.defs {
            let _ = writeln!(text, "  {}: {}", def.line, def.text);
        }
    }
    text
}

/// The map as text: each file, then the line of each definition shown,
/// with a marker for what did not fit.
pub fn render(map: &RepoMap) -> String {
    let mut text = render_files(&map.files);
    if map.files.is_empty() {
        let _ = writeln!(
            text,
            "no definitions found ({} source files read)",
            map.files_scanned
        );
    }
    let plural =
        |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
    let more = match (map.more_defs, map.more_files) {
        (0, 0) => None,
        (defs, 0) => Some(format!(
            "{} in the files above",
            plural(defs, "more definition", "more definitions")
        )),
        (0, files) => Some(plural(files, "more file", "more files")),
        (defs, files) => Some(format!(
            "{} in {}",
            plural(defs, "more definition", "more definitions"),
            plural(files, "more file", "more files")
        )),
    };
    if let Some(more) = more {
        let _ = writeln!(
            text,
            "... {more} (raise --budget, or --focus a file or name)"
        );
    }
    text
}

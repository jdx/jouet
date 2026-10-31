//! The repository map behind `jouet read --map`: a repository's most
//! important definitions, sized to a token budget.
//!
//! The design and its weights are Aider's repo map (Apache-2.0,
//! <https://github.com/Aider-AI/aider/blob/main/aider/repomap.py>), on the
//! graph's edges instead of Aider's tags:
//!
//! - a file that uses a definition (a call, a construction, a type, an
//!   import) gets an edge to the file defining it, weighted ×10 for a
//!   focused identifier or a long distinctive one (snake, kebab or camel
//!   case, 8 characters or more), ×0.1 for a name starting with `_` or
//!   defined in more than 5 files, ×50 when the using file is focused, and
//!   by the square root of the number of uses;
//! - a definition nothing uses still counts a little (a self-edge of 0.1);
//! - personalised PageRank ranks the files, and each file's rank is shared
//!   among the definitions its edges point at;
//! - a binary search finds how many of the ranked definitions fit the
//!   budget.
//!
//! Aider matches names by text, so a call to `load` is evidence for every
//! file defining a `load`. The graph links each use to the definition it
//! means, so nothing has to be shared between same-named definitions, and
//! a use of a field or variant counts for the type that holds it. A
//! file's rank is shared over at least one unit of weight, so a file that
//! uses nothing does not hand all of it to its unused definitions. The
//! definitions of a focused identifier are listed first.

use crate::{Error, Graph};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as _;
use std::path::Path;

/// Aider's default budget, with focus files.
pub const DEFAULT_BUDGET: usize = 1024;
/// Without focus files the map is the agent's whole view of the
/// repository, so Aider multiplies the budget by this.
pub const UNFOCUSED_MULTIPLIER: usize = 8;

/// The kinds of symbol a map lists: what a reader looks for to find their
/// way around, not locals, fields or variants.
const LISTED: &str = "('function', 'method', 'class', 'struct', 'interface', 'trait', 'enum', 'type_alias', 'module', 'macro', 'namespace', 'protocol', 'constant')";

/// Edge kinds that are a use of their target: everything but containment.
const USE_KINDS: &str = "('calls', 'instantiates', 'references', 'imports', 'extends', 'implements', 'type_of', 'returns', 'decorates', 'overrides')";

/// What to map.
#[derive(Debug, Default, Clone)]
pub struct MapOptions {
    /// Files the agent is working on, relative to the work tree. Their own
    /// definitions are left out, as they are already in view.
    pub focus_files: Vec<String>,
    /// Identifiers the agent is interested in.
    pub focus_idents: Vec<String>,
    /// The budget in tokens (estimated at 4 bytes each).
    pub budget: usize,
    /// List only files under this directory, relative to the work tree.
    /// They are still ranked by the whole graph.
    pub within: Option<String>,
}

/// A map, ready to print.
#[derive(Debug, Default, serde::Serialize)]
pub struct RepoMap {
    pub files: Vec<MapFile>,
    /// The map's estimated size in tokens.
    pub tokens: usize,
    /// Files the graph has indexed: what the map was ranked from.
    pub files_ranked: usize,
    /// Ranked definitions and files that did not fit the budget.
    #[serde(skip_serializing_if = "is_zero")]
    pub more_defs: usize,
    #[serde(skip_serializing_if = "is_zero")]
    pub more_files: usize,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct MapFile {
    pub path: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub defs: Vec<MapDef>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct MapDef {
    pub line: u32,
    pub kind: String,
    pub name: String,
    pub text: String,
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

/// One use counted in bulk: how many times the file `from` uses the
/// symbol `to` (a node's `nid`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Uses {
    pub from: String,
    pub to: i64,
    pub count: u32,
}

/// A definition as the map ranks it.
struct Def {
    kind: String,
    name: String,
    path: String,
    line: u32,
}

/// An entry of the ranked list: a definition, or a file with nothing else
/// to show.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Ranked {
    Def(usize),
    File(usize),
}

impl Graph {
    /// Every file's uses of every symbol, in one query: the edges of the
    /// whole graph but containment, counted by using file and target.
    pub(crate) fn uses(&self) -> Result<Vec<Uses>, Error> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT s.file_path, e.target, COUNT(*) FROM edges e JOIN nodes s ON s.nid = e.source
             WHERE e.kind IN {USE_KINDS} GROUP BY s.file_path, e.target"
        ))?;
        let rows = stmt.query_map([], |r| {
            Ok(Uses {
                from: r.get(0)?,
                to: r.get(1)?,
                count: r.get(2)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// The repository map: the files that matter most, each with the line
    /// of its most important definitions, as many as fit the budget.
    pub fn map(&self, options: &MapOptions) -> Result<RepoMap, Error> {
        let files: Vec<String> = self
            .conn
            .prepare("SELECT path FROM files WHERE error IS NULL ORDER BY path")?
            .query_map([], |r| r.get(0))?
            .collect::<Result<_, _>>()?;
        let file_index: HashMap<&str, usize> = files
            .iter()
            .enumerate()
            .map(|(i, f)| (f.as_str(), i))
            .collect();
        // The definitions a map can list, and where every other symbol
        // belongs: a use of a field or a variant counts for its type.
        let mut defs: Vec<Def> = Vec::new();
        let mut def_of: HashMap<i64, usize> = HashMap::new();
        {
            let mut stmt = self.conn.prepare(&format!(
                "SELECT nid, kind, name, file_path, start_line FROM nodes
                 WHERE kind IN {LISTED} OR (kind = 'variable' AND is_exported)
                 ORDER BY file_path, start_line"
            ))?;
            let rows = stmt.query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    Def {
                        kind: r.get(1)?,
                        name: r.get(2)?,
                        path: r.get(3)?,
                        line: r.get(4)?,
                    },
                ))
            })?;
            for row in rows {
                let (nid, def) = row?;
                if file_index.contains_key(def.path.as_str()) {
                    def_of.insert(nid, defs.len());
                    defs.push(def);
                }
            }
        }
        let mut parent: HashMap<i64, i64> = HashMap::new();
        {
            let mut stmt = self
                .conn
                .prepare("SELECT source, target FROM edges WHERE kind = 'contains'")?;
            for row in stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))? {
                let (source, target) = row?;
                parent.insert(target, source);
            }
        }
        let listed = |mut nid: i64| -> Option<usize> {
            for _ in 0..8 {
                if let Some(&d) = def_of.get(&nid) {
                    return Some(d);
                }
                nid = *parent.get(&nid)?;
            }
            None
        };
        let mut counts: HashMap<(usize, usize), u32> = HashMap::new();
        for u in self.uses()? {
            let (Some(&from), Some(to)) = (file_index.get(u.from.as_str()), listed(u.to)) else {
                continue;
            };
            *counts.entry((from, to)).or_default() += u.count;
        }
        let ranked = rank(&files, &defs, &counts, options);
        let mut map = fit(&self.root, &ranked, &files, &defs, options.budget);
        map.files_ranked = files.len();
        Ok(map)
    }
}

/// Aider's weight for uses of a name.
fn weight(name: &str, focused: bool, definers: usize) -> f64 {
    let mut mul = 1.0;
    let has_alpha = name.chars().any(char::is_alphabetic);
    let snake = name.contains('_') && has_alpha;
    let kebab = name.contains('-') && has_alpha;
    let camel = name.chars().any(char::is_uppercase) && name.chars().any(char::is_lowercase);
    if focused {
        mul *= 10.0;
    }
    if (snake || kebab || camel) && name.chars().count() >= 8 {
        mul *= 10.0;
    }
    if name.starts_with('_') {
        mul *= 0.1;
    }
    if definers > 5 {
        mul *= 0.1;
    }
    mul
}

/// Ranks every definition, most important first, following Aider's
/// `get_ranked_tags`.
fn rank(
    files: &[String],
    defs: &[Def],
    counts: &HashMap<(usize, usize), u32>,
    options: &MapOptions,
) -> Vec<Ranked> {
    let file_index: HashMap<&str, usize> = files
        .iter()
        .enumerate()
        .map(|(i, f)| (f.as_str(), i))
        .collect();
    let focus_idents: HashSet<&str> = options.focus_idents.iter().map(String::as_str).collect();
    let focused: Vec<bool> = files
        .iter()
        .map(|f| options.focus_files.iter().any(|x| x == f))
        .collect();
    let mut personal = vec![0.0; files.len()];
    let personalize = 100.0 / files.len().max(1) as f64;
    for (id, file) in files.iter().enumerate() {
        if focused[id] {
            personal[id] += personalize;
        }
        // A file whose path names a focused identifier counts too.
        let path = Path::new(file);
        let mut parts: HashSet<&str> = path.iter().filter_map(|p| p.to_str()).collect();
        parts.extend(path.file_stem().and_then(|s| s.to_str()));
        if !focus_idents.is_empty() && parts.iter().any(|p| focus_idents.contains(p)) {
            personal[id] += personalize;
        }
    }
    let mut definers: HashMap<&str, HashSet<&str>> = HashMap::new();
    for d in defs {
        definers.entry(&d.name).or_default().insert(&d.path);
    }
    // The edges from each file to each definition it uses, weighted.
    let mut edges: Vec<(usize, usize, f64)> = Vec::new();
    let mut used = vec![false; defs.len()];
    let mut keys: Vec<(&(usize, usize), &u32)> = counts.iter().collect();
    keys.sort_unstable();
    for (&(from, to), &count) in keys {
        let d = &defs[to];
        let mut w = weight(
            &d.name,
            focus_idents.contains(d.name.as_str()),
            definers[d.name.as_str()].len(),
        ) * f64::from(count).sqrt();
        if focused[from] {
            w *= 50.0;
        }
        edges.push((from, to, w));
        used[to] = true;
    }
    for (i, d) in defs.iter().enumerate() {
        if !used[i] {
            edges.push((file_index[d.path.as_str()], i, 0.1));
        }
    }
    let target = |to: usize| file_index[defs[to].path.as_str()];
    let ranks = pagerank(
        files.len(),
        edges.iter().map(|&(from, to, w)| (from, target(to), w)),
        &personal,
    );
    // Share each file's rank among the definitions its edges point at,
    // over at least one unit of weight: a file that uses nothing would
    // otherwise hand all of its rank to its unused definitions' 0.1.
    let mut out_weight = vec![1.0f64; files.len()];
    let mut used_weight = vec![0.0f64; files.len()];
    for &(from, _, w) in &edges {
        used_weight[from] += w;
    }
    for (o, u) in out_weight.iter_mut().zip(&used_weight) {
        *o = o.max(*u);
    }
    let mut def_rank = vec![0.0; defs.len()];
    for &(from, to, w) in &edges {
        def_rank[to] += ranks[from] * w / out_weight[from];
    }
    let mut order: Vec<usize> = (0..defs.len()).collect();
    order.sort_by(|&a, &b| def_rank[b].total_cmp(&def_rank[a]).then(a.cmp(&b)));
    let shown: Vec<bool> = files
        .iter()
        .enumerate()
        .map(|(i, f)| {
            !focused[i]
                && options.within.as_deref().is_none_or(|w| {
                    f.strip_prefix(w.trim_end_matches('/'))
                        .is_some_and(|rest| rest.starts_with('/'))
                })
        })
        .collect();
    let mut ranked = Vec::new();
    let mut included = vec![false; files.len()];
    for d in order {
        let file = target(d);
        if !shown[file] {
            continue;
        }
        ranked.push(Ranked::Def(d));
        included[file] = true;
    }
    // Definitions of a focused identifier come first: the agent named it.
    if !focus_idents.is_empty() {
        let named = |r: &Ranked| matches!(r, Ranked::Def(d) if focus_idents.contains(defs[*d].name.as_str()));
        let (first, rest): (Vec<Ranked>, Vec<Ranked>) = ranked.into_iter().partition(named);
        ranked = first.into_iter().chain(rest).collect();
    }
    // Then files by their own rank, so the map can name a file it has no
    // room to show the definitions of.
    let mut by_rank: Vec<usize> = (0..files.len()).collect();
    by_rank.sort_by(|&a, &b| ranks[b].total_cmp(&ranks[a]).then(a.cmp(&b)));
    for id in by_rank {
        if !included[id] && shown[id] {
            ranked.push(Ranked::File(id));
        }
    }
    ranked
}

/// Personalised PageRank over `n` nodes, as networkx computes it: damping
/// 0.85, at most 100 iterations, and dangling nodes' rank spread by the
/// personalisation (or evenly without one).
fn pagerank(
    n: usize,
    edges: impl Iterator<Item = (usize, usize, f64)>,
    personal: &[f64],
) -> Vec<f64> {
    const ALPHA: f64 = 0.85;
    const TOLERANCE: f64 = 1.0e-6;
    if n == 0 {
        return Vec::new();
    }
    let edges: Vec<(usize, usize, f64)> = edges.collect();
    let total: f64 = personal.iter().sum();
    let p: Vec<f64> = if total > 0.0 {
        personal.iter().map(|v| v / total).collect()
    } else {
        vec![1.0 / n as f64; n]
    };
    let mut out_weight = vec![0.0; n];
    for &(from, _, w) in &edges {
        out_weight[from] += w;
    }
    let mut x = vec![1.0 / n as f64; n];
    for _ in 0..100 {
        let last = x.clone();
        x.iter_mut().for_each(|v| *v = 0.0);
        let dangling: f64 = ALPHA
            * (0..n)
                .filter(|&i| out_weight[i] <= 0.0)
                .map(|i| last[i])
                .sum::<f64>();
        for &(from, to, w) in &edges {
            x[to] += ALPHA * last[from] * w / out_weight[from];
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

/// The first line of each listed definition, read once per file.
struct Lines<'a> {
    root: &'a Path,
    files: HashMap<&'a str, Vec<String>>,
}

impl<'a> Lines<'a> {
    fn line(&mut self, path: &'a str, line: u32) -> String {
        let root = self.root;
        let lines = self.files.entry(path).or_insert_with(|| {
            std::fs::read_to_string(root.join(path))
                .map(|t| t.lines().map(str::to_string).collect())
                .unwrap_or_default()
        });
        lines
            .get(line.saturating_sub(1) as usize)
            .map(|l| clip(l.trim_end()))
            .unwrap_or_default()
    }
}

/// A line cut to 200 bytes, at a character boundary, with a marker.
fn clip(line: &str) -> String {
    const MAX: usize = 200;
    if line.len() <= MAX {
        return line.to_string();
    }
    let mut end = MAX;
    while !line.is_char_boundary(end) {
        end -= 1;
    }
    format!("{} … ({} more bytes)", &line[..end], line.len() - end)
}

/// The largest prefix of the ranked list whose rendering fits the budget,
/// found by binary search, as Aider does.
fn fit(root: &Path, ranked: &[Ranked], files: &[String], defs: &[Def], budget: usize) -> RepoMap {
    let mut lines = Lines {
        root,
        files: HashMap::new(),
    };
    let mut best = (0, assemble(&[], files, defs, &mut lines));
    let (mut low, mut high) = (0, ranked.len());
    // Aider's first guess: about 25 tokens a definition.
    let mut middle = (budget / 25).min(ranked.len());
    while low <= high {
        let map = assemble(&ranked[..middle], files, defs, &mut lines);
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
    map.more_defs = rest.iter().filter(|r| matches!(r, Ranked::Def(_))).count();
    let in_map: HashSet<&str> = map.files.iter().map(|f| f.path.as_str()).collect();
    let missing: HashSet<&str> = rest
        .iter()
        .map(|r| match *r {
            Ranked::Def(d) => defs[d].path.as_str(),
            Ranked::File(f) => files[f].as_str(),
        })
        .filter(|file| !in_map.contains(file))
        .collect();
    map.more_files = missing.len();
    map
}

/// The map for a prefix of the ranked list: its files in path order, each
/// with its definitions in line order.
fn assemble<'a>(
    ranked: &[Ranked],
    files: &'a [String],
    defs: &'a [Def],
    lines: &mut Lines<'a>,
) -> RepoMap {
    let mut chosen: BTreeMap<&str, BTreeMap<(u32, usize), &Def>> = BTreeMap::new();
    for r in ranked {
        match *r {
            Ranked::Def(d) => {
                let def = &defs[d];
                chosen
                    .entry(&def.path)
                    .or_default()
                    .insert((def.line, d), def);
            }
            Ranked::File(f) => {
                chosen.entry(&files[f]).or_default();
            }
        }
    }
    let mut map = RepoMap {
        files: chosen
            .into_iter()
            .map(|(path, defs)| MapFile {
                path: path.to_string(),
                defs: defs
                    .into_values()
                    .map(|d| MapDef {
                        line: d.line,
                        kind: d.kind.clone(),
                        name: d.name.clone(),
                        text: lines.line(&d.path, d.line),
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
        let mut last = None;
        for def in &file.defs {
            // Two definitions on one line (a struct and its impl's
            // constructor, written on one line) are shown once.
            if last == Some(def.line) {
                continue;
            }
            last = Some(def.line);
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
            "no definitions found ({} files in the code graph)",
            map.files_ranked
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

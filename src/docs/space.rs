//! Public paths through a package's modules and re-exports, for the
//! languages whose public API is a module tree (Rust and Python).
//!
//! Each module lists what it defines publicly, its child modules and its
//! re-exports (`pub use`, `from .x import Y`). A walk from the root modules
//! gives every definition each path it is reachable by, the shortest first,
//! following re-exports through any number of modules of the package (so
//! `reqwest::Client` reaches `async_impl/client.rs` through two `pub use`s)
//! but never into another package: those are listed as re-exports.

use super::Item;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

/// How many re-exports one name is followed through.
const MAX_HOPS: usize = 8;

/// The deepest public path kept.
const MAX_DEPTH: usize = 10;

pub(super) struct Module {
    /// From the package's root name: `["reqwest", "blocking"]`.
    pub(super) path: Vec<String>,
    /// Declared public itself (its parents may not be).
    pub(super) public: bool,
    /// Its public definitions, by name.
    pub(super) exports: Vec<(String, usize)>,
    /// Every definition, public or not, by name: what a re-export can name.
    pub(super) defs: HashMap<String, usize>,
    /// Child modules by name, public or not.
    pub(super) children: BTreeMap<String, usize>,
    pub(super) uses: Vec<Use>,
    /// The module's own item, which gets the module's paths.
    pub(super) item: Option<usize>,
}

impl Module {
    pub(super) fn new(path: Vec<String>, public: bool) -> Module {
        Module {
            path,
            public,
            exports: Vec::new(),
            defs: HashMap::new(),
            children: BTreeMap::new(),
            uses: Vec::new(),
            item: None,
        }
    }
}

/// A re-export.
pub(super) enum Use {
    /// `name` from the package's module at `module`, exposed as `alias`.
    Name {
        module: Vec<String>,
        name: String,
        alias: String,
    },
    /// Everything public from the package's module at `module`.
    Glob { module: Vec<String> },
    /// Another package's definition, exposed as `alias`; `text` is the
    /// declaration, at `file:line` of the package.
    External {
        alias: String,
        text: String,
        file: String,
        line: u32,
    },
}

#[derive(Clone)]
enum Target {
    Item(usize),
    Module(usize),
    /// The declaration and where it is.
    External(String, String, u32),
}

struct Walk<'a> {
    modules: &'a [Module],
    by_path: HashMap<&'a [String], usize>,
}

impl Walk<'_> {
    /// What `name` in module `m` refers to, through re-exports.
    fn resolve(&self, m: usize, name: &str, hops: usize) -> Option<Target> {
        if hops > MAX_HOPS {
            return None;
        }
        let module = &self.modules[m];
        if let Some(&i) = module.defs.get(name) {
            return Some(Target::Item(i));
        }
        // A re-export before a child module of the same name: Rust keeps
        // modules and functions apart, and tokio's `task` has both a private
        // `mod spawn` and `pub use spawn::spawn`.
        for u in &module.uses {
            match u {
                Use::Name {
                    module: from,
                    name: target,
                    alias,
                } if alias == name => {
                    if let Some(&at) = self.by_path.get(from.as_slice())
                        && let Some(found) = self.resolve(at, target, hops + 1)
                    {
                        return Some(found);
                    }
                }
                Use::External {
                    alias,
                    text,
                    file,
                    line,
                } if alias == name => {
                    return Some(Target::External(text.clone(), file.clone(), *line));
                }
                _ => {}
            }
        }
        if let Some(&child) = module.children.get(name) {
            return Some(Target::Module(child));
        }
        module.uses.iter().find_map(|u| match u {
            Use::Glob { module: from } => {
                let at = *self.by_path.get(from.as_slice())?;
                (at != m)
                    .then(|| self.resolve(at, name, hops + 1))
                    .flatten()
            }
            _ => None,
        })
    }

    /// Everything module `m` exposes, by name.
    fn exposed(&self, m: usize, hops: usize) -> Vec<(String, Target)> {
        let module = &self.modules[m];
        let mut out: Vec<(String, Target)> = module
            .exports
            .iter()
            .map(|(name, i)| (name.clone(), Target::Item(*i)))
            .collect();
        out.extend(
            module
                .children
                .iter()
                .filter(|(_, c)| self.modules[**c].public)
                .map(|(name, c)| (name.clone(), Target::Module(*c))),
        );
        for u in &module.uses {
            match u {
                Use::Name {
                    module: from,
                    name,
                    alias,
                } => {
                    if let Some(&at) = self.by_path.get(from.as_slice())
                        && let Some(target) = self.resolve(at, name, 1)
                    {
                        out.push((alias.clone(), target));
                    }
                }
                Use::External {
                    alias,
                    text,
                    file,
                    line,
                } => {
                    out.push((
                        alias.clone(),
                        Target::External(text.clone(), file.clone(), *line),
                    ));
                }
                Use::Glob { module: from } => {
                    if hops < MAX_HOPS
                        && let Some(&at) = self.by_path.get(from.as_slice())
                        && at != m
                    {
                        out.extend(self.exposed(at, hops + 1));
                    }
                }
            }
        }
        out
    }
}

/// Gives each item every public path that reaches it from `roots`, and adds
/// an item for each re-export of another package's definition.
pub(super) fn assign(modules: &[Module], roots: &[usize], items: &mut Vec<Item>, sep: &str) {
    let walk = Walk {
        modules,
        by_path: modules
            .iter()
            .enumerate()
            .map(|(i, m)| (m.path.as_slice(), i))
            .collect(),
    };
    let mut seen: HashSet<usize> = HashSet::new();
    let mut queue: VecDeque<(usize, Vec<String>)> = roots
        .iter()
        .map(|&r| (r, modules[r].path.clone()))
        .collect();
    let mut external: HashMap<String, usize> = HashMap::new();
    // One re-export per path: `#[cfg]` variants of `pub use x as main`.
    let mut external_paths: HashSet<String> = HashSet::new();
    while let Some((m, prefix)) = queue.pop_front() {
        if !seen.insert(m) || prefix.len() >= MAX_DEPTH {
            continue;
        }
        if let Some(i) = modules[m].item {
            items[i].paths.push(prefix.join(sep));
        }
        for (name, target) in walk.exposed(m, 0) {
            let path = format!("{}{sep}{name}", prefix.join(sep));
            match target {
                Target::Item(i) => items[i].paths.push(path),
                Target::Module(c) => {
                    let mut next = prefix.clone();
                    next.push(name);
                    if seen.contains(&c) {
                        if let Some(i) = modules[c].item {
                            items[i].paths.push(path);
                        }
                    } else {
                        queue.push_back((c, next));
                    }
                }
                Target::External(text, file, line) => {
                    if !external_paths.insert(path.clone()) {
                        continue;
                    }
                    if let Some(&i) = external.get(&text) {
                        items[i].paths.push(path);
                        continue;
                    }
                    let mut item = Item::new("reexport", &name, &file, (line, line));
                    item.signature = text.clone();
                    item.paths.push(path);
                    external.insert(text, items.len());
                    items.push(item);
                }
            }
        }
    }
}

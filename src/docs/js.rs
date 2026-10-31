//! An npm package's API: the exported declarations of its `.d.ts` files,
//! the entry its `package.json` names first, with the members of its
//! classes and interfaces. A package without type declarations is read
//! from its JavaScript instead.

use super::text::{self, Cut, Style};
use super::{Index, Item, Package};
use std::collections::{HashMap, HashSet};
use std::path::Path;

const SKIP_DIRS: [&str; 5] = ["node_modules", "test", "tests", "__tests__", "coverage"];

const CONTAINERS: [&str; 5] = ["class", "interface", "enum", "namespace", "module"];
const TOP_KINDS: [&str; 10] = [
    "class",
    "interface",
    "enum",
    "namespace",
    "module",
    "function",
    "type_alias",
    "variable",
    "constant",
    "component",
];

fn is_declaration(name: &str) -> bool {
    [".d.ts", ".d.mts", ".d.cts"]
        .iter()
        .any(|e| name.ends_with(e))
}

fn is_script(name: &str) -> bool {
    !name.ends_with(".min.js")
        && [".js", ".mjs", ".cjs", ".ts", ".mts", ".cts", ".jsx", ".tsx"]
            .iter()
            .any(|e| name.ends_with(e))
}

fn manifest(root: &Path) -> Option<serde_json::Value> {
    serde_json::from_str(&std::fs::read_to_string(root.join("package.json")).ok()?).ok()
}

/// Whether the package ships type declarations.
pub(super) fn has_types(root: &Path) -> bool {
    entry(root).is_some_and(|e| is_declaration(&e))
        || !super::walk(root, is_declaration, |name| !SKIP_DIRS.contains(&name)).is_empty()
}

/// The declaration file `package.json` names (`types`, `typings`, or the
/// `types` condition of `exports["."]`), else the `main` script.
fn entry(root: &Path) -> Option<String> {
    let value = manifest(root)?;
    let dot = value.get("exports").and_then(|e| e.get(".").or(Some(e)));
    let conditions = [
        value.get("types"),
        value.get("typings"),
        dot.and_then(|d| d.get("types")),
        dot.and_then(|d| d.get("import"))
            .and_then(|i| i.get("types")),
        dot.and_then(|d| d.get("require"))
            .and_then(|r| r.get("types")),
        value.get("module"),
        value.get("main"),
    ];
    conditions
        .into_iter()
        .flatten()
        .find_map(|v| v.as_str())
        .map(|s| s.trim_start_matches("./").to_string())
}

pub(super) fn build(pkg: &Package) -> Result<Index, String> {
    let root = &pkg.api_root;
    let declarations = super::walk(root, is_declaration, |name| !SKIP_DIRS.contains(&name));
    let mut rels = if declarations.is_empty() {
        super::walk(root, is_script, |name| !SKIP_DIRS.contains(&name))
    } else {
        declarations
    };
    if let Some(entry) = entry(root)
        && let Some(at) = rels.iter().position(|r| *r == entry)
    {
        let first = rels.remove(at);
        rels.insert(0, first);
    }
    let order: HashMap<String, usize> = rels
        .iter()
        .enumerate()
        .map(|(i, r)| (r.clone(), i))
        .collect();
    let mut sources = super::parse(root, rels);
    sources.sort_by_key(|s| order.get(&s.rel).copied().unwrap_or(usize::MAX));

    let mut items: Vec<Item> = Vec::new();
    let mut seen: HashSet<(String, String)> = HashSet::new();
    for source in &sources {
        let lines = source.lines();
        let exports_anything = source.text.contains("export ");
        let mut owners: HashMap<String, usize> = HashMap::new();
        let mut here: HashSet<(String, String)> = HashSet::new();
        for node in &source.nodes {
            let segments: Vec<&str> = node.qualified_name.split("::").collect();
            let start = node.start_line as usize;
            let container = CONTAINERS.contains(&node.kind);
            let cut = if container { Cut::Body } else { Cut::Short };
            let mut item = Item::new(
                node.kind,
                &node.name,
                &source.rel,
                (node.start_line, node.end_line),
            );
            item.signature = text::header(&lines, start, node.end_line as usize, cut);
            item.doc = text::doc_above(&lines, start, Style::Js)
                .0
                .or_else(|| node.docstring.clone());
            if segments.len() == 1 && TOP_KINDS.contains(&node.kind) {
                let key = (node.kind.to_string(), node.name.clone());
                // The same declaration in another file (`index.d.ts` and
                // `index.d.mts`) is listed once; overloads in one file stay.
                if seen.contains(&key) && !here.contains(&key) {
                    continue;
                }
                item.public = node.is_exported || !exports_anything;
                item.home = node.name.clone();
                if item.public {
                    item.paths.push(node.name.clone());
                }
                seen.insert(key.clone());
                here.insert(key);
                if container {
                    owners.entry(node.name.clone()).or_insert(items.len());
                }
                items.push(item);
            } else if segments.len() == 2
                && let Some(&owner) = owners.get(segments[0])
            {
                if matches!(node.visibility, Some("private" | "protected"))
                    || node.name.starts_with('#')
                {
                    continue;
                }
                item.owner = Some(owner);
                item.public = true;
                item.home = format!("{}.{}", segments[0], node.name);
                items.push(item);
            }
        }
    }
    super::finish(&mut items, ".");
    super::sort_paths(&mut items);
    let doc = manifest(&pkg.root).and_then(|m| m.get("description")?.as_str().map(str::to_string));
    Ok(Index {
        format: 0,
        doc,
        items,
    })
}

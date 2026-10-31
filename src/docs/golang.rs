//! A Go package's API: the exported (capitalised) declarations of the
//! package directory's `.go` files, without tests, and the exported
//! methods of its types.

use super::text::{self, Cut, Style};
use super::{Index, Item, Package};
use std::collections::HashMap;

const TOP_KINDS: [&str; 8] = [
    "function",
    "struct",
    "interface",
    "type_alias",
    "class",
    "enum",
    "constant",
    "variable",
];
const TYPE_KINDS: [&str; 5] = ["struct", "interface", "type_alias", "class", "enum"];

fn exported(name: &str) -> bool {
    name.chars().next().is_some_and(char::is_uppercase)
}

pub(super) fn build(pkg: &Package) -> Result<Index, String> {
    let dir = &pkg.api_root;
    let mut rels: Vec<String> = std::fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".go") && !n.ends_with("_test.go"))
        .collect();
    rels.sort();
    let sources = super::parse(dir, rels);
    let package = sources
        .iter()
        .find_map(|s| {
            s.text
                .lines()
                .find_map(|l| l.strip_prefix("package ").map(|p| p.trim().to_string()))
        })
        .unwrap_or_else(|| pkg.name.rsplit('/').next().unwrap_or(&pkg.name).to_string());
    let mut items: Vec<Item> = Vec::new();
    let mut types: HashMap<String, usize> = HashMap::new();
    let mut methods: Vec<(usize, String)> = Vec::new();
    let mut doc = None;
    for source in &sources {
        let lines = source.lines();
        if doc.is_none()
            && let Some(at) = lines.iter().position(|l| l.starts_with("package "))
        {
            doc = text::doc_above(&lines, at + 1, Style::Go).0;
        }
        for node in &source.nodes {
            let segments: Vec<&str> = node.qualified_name.split("::").collect();
            let start = node.start_line as usize;
            let mut item = Item::new(
                node.kind,
                &node.name,
                &source.rel,
                (node.start_line, node.end_line),
            );
            let cut = if matches!(node.kind, "constant" | "variable" | "field") {
                Cut::Short
            } else {
                Cut::Body
            };
            item.signature = text::header(&lines, start, node.end_line as usize, cut);
            item.doc = text::doc_above(&lines, start, Style::Go)
                .0
                .or_else(|| node.docstring.clone());
            item.public = exported(&node.name);
            match segments.as_slice() {
                [_] if TOP_KINDS.contains(&node.kind) => {
                    item.home = format!("{package}.{}", node.name);
                    if item.public {
                        item.paths.push(item.home.clone());
                    }
                    if TYPE_KINDS.contains(&node.kind) {
                        types.entry(node.name.clone()).or_insert(items.len());
                    }
                    items.push(item);
                }
                [owner, _] if matches!(node.kind, "method" | "field" | "property") => {
                    item.home = format!("{package}.{owner}.{}", node.name);
                    methods.push((items.len(), (*owner).to_string()));
                    items.push(item);
                }
                _ => {}
            }
        }
    }
    for (idx, owner) in methods {
        match types.get(&owner) {
            Some(&t) => items[idx].owner = Some(t),
            None => items[idx].public = false,
        }
    }
    super::finish(&mut items, ".");
    super::sort_paths(&mut items);
    Ok(Index {
        format: 0,
        doc,
        items,
    })
}

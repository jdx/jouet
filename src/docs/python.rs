//! A Python distribution's public API: the modules and names that do not
//! start with `_`, or the names a module's `__all__` lists when it has one,
//! with re-exports followed (`from .core import Context` in `__init__.py`
//! makes `click.Context`). A `.pyi` stub is read instead of its `.py`.

use super::space::{self, Module, Use};
use super::text::{self, Cut};
use super::{Index, Item, Package};
use regex::Regex;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::OnceLock;

const SKIP_DIRS: [&str; 5] = ["__pycache__", "tests", "test", "testing", "node_modules"];

/// Members that start with `_` but are part of a class's interface.
const DUNDERS: [&str; 2] = ["__init__", "__call__"];

pub(super) fn build(pkg: &Package) -> Result<Index, String> {
    let site = &pkg.api_root;
    let mut rels: Vec<String> = Vec::new();
    for top in &pkg.modules {
        let dir = site.join(top);
        if dir.is_dir() {
            rels.extend(
                super::walk(
                    &dir,
                    |name| name.ends_with(".py") || name.ends_with(".pyi"),
                    |name| !SKIP_DIRS.contains(&name),
                )
                .into_iter()
                .map(|r| format!("{top}/{r}")),
            );
        } else {
            for ext in ["pyi", "py"] {
                let file = format!("{top}.{ext}");
                if site.join(&file).is_file() {
                    rels.push(file);
                    break;
                }
            }
        }
    }
    // A stub stands for its module.
    let stubs: HashSet<String> = rels
        .iter()
        .filter_map(|r| r.strip_suffix(".pyi").map(str::to_string))
        .collect();
    rels.retain(|r| {
        r.strip_suffix(".py")
            .is_none_or(|stem| !stubs.contains(stem))
    });
    let sources = super::parse(site, rels);

    let mut modules: Vec<Module> = Vec::new();
    let mut by_path: BTreeMap<Vec<String>, usize> = BTreeMap::new();
    let mut items: Vec<Item> = Vec::new();
    let mut doc = None;
    let mut ensure = |path: &[String], modules: &mut Vec<Module>| -> usize {
        // Parents first, so every module is its parent's child.
        let mut id = 0;
        for depth in 1..=path.len() {
            let prefix = path[..depth].to_vec();
            if let Some(&found) = by_path.get(&prefix) {
                id = found;
                continue;
            }
            id = modules.len();
            let public = !prefix.last().is_some_and(|s| s.starts_with('_'));
            modules.push(Module::new(prefix.clone(), public));
            if depth > 1
                && let Some(&parent) = by_path.get(&prefix[..depth - 1])
            {
                modules[parent]
                    .children
                    .insert(prefix[depth - 1].clone(), id);
            }
            by_path.insert(prefix, id);
        }
        id
    };
    let mut file_modules = Vec::new();
    for (s, source) in sources.iter().enumerate() {
        let path = module_path(&source.rel);
        if path.is_empty() || path.last().is_some_and(|l| l == "__main__") {
            continue;
        }
        let m = ensure(&path, &mut modules);
        let is_package =
            source.rel.ends_with("__init__.py") || source.rel.ends_with("__init__.pyi");
        file_modules.push((s, m, is_package));
    }
    for &(s, m, is_package) in &file_modules {
        let source = &sources[s];
        let lines = source.lines();
        let path = modules[m].path.clone();
        let module_doc = text::python_module_doc(&source.text);
        if path.len() == 1 && doc.is_none() {
            doc.clone_from(&module_doc);
        }
        let mut module_item = Item::new(
            "module",
            path.last().map_or("", String::as_str),
            &source.rel,
            (1, lines.len() as u32),
        );
        module_item.signature = format!("module {}", path.join("."));
        module_item.doc = module_doc;
        module_item.home = path.join(".");
        modules[m].item = Some(items.len());
        items.push(module_item);

        let all = dunder_all(&source.text);
        let public_name = |name: &str| match &all {
            Some(all) => all.iter().any(|a| a == name),
            None => !name.starts_with('_'),
        };
        let mut classes: HashMap<String, usize> = HashMap::new();
        for node in &source.nodes {
            let segments: Vec<&str> = node.qualified_name.split("::").collect();
            let start = node.start_line as usize;
            let mut item = Item::new(
                node.kind,
                &node.name,
                &source.rel,
                (node.start_line, node.end_line),
            );
            item.signature = text::header(&lines, start, node.end_line as usize, Cut::Colon);
            item.doc.clone_from(&node.docstring);
            match (segments.len(), node.kind) {
                (1, "class" | "function") => {}
                (1, "variable" | "constant") => {
                    let listed = all
                        .as_ref()
                        .is_some_and(|a| a.iter().any(|n| n == &node.name));
                    let constant = node
                        .name
                        .chars()
                        .all(|c| c.is_ascii_uppercase() || c == '_' || c.is_ascii_digit());
                    if !listed && !constant {
                        continue;
                    }
                    item.signature =
                        text::header(&lines, start, node.end_line as usize, Cut::Short);
                }
                (2, "method" | "function" | "property") => {
                    let Some(&owner) = classes.get(segments[0]) else {
                        continue;
                    };
                    item.kind = "method".into();
                    item.owner = Some(owner);
                    item.public =
                        !node.name.starts_with('_') || DUNDERS.contains(&node.name.as_str());
                    item.home = format!("{}.{}.{}", path.join("."), segments[0], node.name);
                    items.push(item);
                    continue;
                }
                _ => continue,
            }
            item.home = format!("{}.{}", path.join("."), node.name);
            let idx = items.len();
            if node.kind == "class" {
                classes.insert(node.name.clone(), idx);
            }
            items.push(item);
            modules[m].defs.entry(node.name.clone()).or_insert(idx);
            if public_name(&node.name) {
                modules[m].exports.push((node.name.clone(), idx));
            }
        }
        let package: Vec<String> = if is_package {
            path.clone()
        } else {
            path[..path.len() - 1].to_vec()
        };
        for import in imports(&lines) {
            let Some(from) = resolve(&import.module, &package, &pkg.modules) else {
                // Another package's name, re-exported only when declared so.
                for (name, alias) in &import.names {
                    if all.as_ref().is_some_and(|a| a.contains(alias))
                        || name == alias && import.explicit
                    {
                        modules[m].uses.push(Use::External {
                            alias: alias.clone(),
                            text: format!("from {} import {name}", import.module),
                            file: source.rel.clone(),
                            line: import.line as u32 + 1,
                        });
                    }
                }
                continue;
            };
            if import.names.is_empty() {
                if is_package {
                    modules[m].uses.push(Use::Glob { module: from });
                }
                continue;
            }
            for (name, alias) in import.names {
                let exported = match &all {
                    Some(all) => all.contains(&alias),
                    None => {
                        is_package && !alias.starts_with('_') || import.explicit && name == alias
                    }
                };
                if exported {
                    modules[m].uses.push(Use::Name {
                        module: from.clone(),
                        name,
                        alias,
                    });
                }
            }
        }
    }
    let roots: Vec<usize> = pkg
        .modules
        .iter()
        .filter_map(|top| by_path.get(std::slice::from_ref(top)).copied())
        .collect();
    space::assign(&modules, &roots, &mut items, ".");
    super::finish(&mut items, ".");
    super::sort_paths(&mut items);
    Ok(Index {
        format: 0,
        doc,
        items,
    })
}

/// `click/core.py` is `["click", "core"]`; `click/__init__.py` is
/// `["click"]`.
fn module_path(rel: &str) -> Vec<String> {
    let stem = rel
        .strip_suffix(".pyi")
        .or_else(|| rel.strip_suffix(".py"))
        .unwrap_or(rel);
    let mut parts: Vec<String> = stem.split('/').map(str::to_string).collect();
    if parts.last().is_some_and(|l| l == "__init__") {
        parts.pop();
    }
    parts
}

/// The names a module's `__all__` lists, when it lists them literally.
fn dunder_all(text: &str) -> Option<Vec<String>> {
    static ALL: OnceLock<Regex> = OnceLock::new();
    static NAME: OnceLock<Regex> = OnceLock::new();
    let all = ALL
        .get_or_init(|| Regex::new(r"(?ms)^__all__\s*(?::[^=\n]*)?=\s*[\[(](.*?)[\])]").unwrap());
    let name = NAME.get_or_init(|| Regex::new(r#"["']([A-Za-z_]\w*)["']"#).unwrap());
    let body = all.captures(text)?;
    Some(
        name.captures_iter(&body[1])
            .map(|c| c[1].to_string())
            .collect(),
    )
}

/// A `from X import ...` statement.
struct Import {
    /// 0-based.
    line: usize,
    /// `X`, with its leading dots.
    module: String,
    /// (name, alias); empty for `*`.
    names: Vec<(String, String)>,
    /// Written `import A as A`, the stub convention for a re-export.
    explicit: bool,
}

fn imports(lines: &[&str]) -> Vec<Import> {
    static FROM: OnceLock<Regex> = OnceLock::new();
    let from = FROM.get_or_init(|| Regex::new(r"^\s*from\s+(\.*[\w.]*)\s+import\s+(.*)$").unwrap());
    let mut out = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let Some(c) = from.captures(line) else {
            continue;
        };
        let mut rest = c[2].split('#').next().unwrap_or("").trim().to_string();
        if rest.starts_with('(') && !rest.contains(')') {
            for next in &lines[i + 1..lines.len().min(i + 200)] {
                rest.push(' ');
                rest.push_str(next.split('#').next().unwrap_or(""));
                if next.contains(')') {
                    break;
                }
            }
        }
        let rest =
            rest.trim_matches(|c: char| c == '(' || c == ')' || c.is_whitespace() || c == '\\');
        let mut names = Vec::new();
        let mut explicit = false;
        if rest != "*" {
            for part in rest.split(',') {
                let words: Vec<&str> = part.split_whitespace().collect();
                match words.as_slice() {
                    [name] => names.push((name.to_string(), name.to_string())),
                    [name, "as", alias] => {
                        explicit |= name == alias;
                        names.push((name.to_string(), alias.to_string()));
                    }
                    _ => {}
                }
            }
            if names.is_empty() {
                continue;
            }
        }
        out.push(Import {
            line: i,
            module: c[1].to_string(),
            names,
            explicit,
        });
    }
    out
}

/// The module path an import names, from within `package`, when it is one
/// of this distribution's.
fn resolve(module: &str, package: &[String], tops: &[String]) -> Option<Vec<String>> {
    let dots = module.chars().take_while(|&c| c == '.').count();
    let rest: Vec<String> = module[dots..]
        .split('.')
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    if dots == 0 {
        return tops.contains(rest.first()?).then_some(rest);
    }
    let keep = package.len().checked_sub(dots - 1)?;
    let mut out = package[..keep].to_vec();
    out.extend(rest);
    (!out.is_empty()).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imports_are_read_and_resolved() {
        let src = "from .core import Context as Context\nfrom . import (\n    utils,\n    types as t,\n)\nfrom typing import Any\nfrom .x import *\n";
        let lines: Vec<&str> = src.lines().collect();
        let found = imports(&lines);
        assert_eq!(found.len(), 4);
        assert!(found[0].explicit);
        assert_eq!(
            found[1].names,
            [
                ("utils".into(), "utils".into()),
                ("types".into(), "t".into())
            ]
        );
        assert!(found[3].names.is_empty());
        let pkg = vec!["click".to_string()];
        let tops = pkg.clone();
        assert_eq!(
            resolve(".core", &pkg, &tops),
            Some(vec!["click".into(), "core".into()])
        );
        assert_eq!(
            resolve("..x", &["a".into(), "b".into()], &tops),
            Some(vec!["a".into(), "x".into()])
        );
        assert_eq!(
            resolve("click.core", &pkg, &tops),
            Some(vec!["click".into(), "core".into()])
        );
        assert_eq!(resolve("typing", &pkg, &tops), None);
    }

    #[test]
    fn dunder_all_is_read() {
        assert_eq!(
            dunder_all("x = 1\n__all__ = [\n    \"a\",\n    'b',\n]\n"),
            Some(vec!["a".into(), "b".into()])
        );
        assert_eq!(dunder_all("x = 1\n"), None);
    }
}

//! A crate's public API: the items declared `pub` (not `pub(crate)`, not
//! `#[doc(hidden)]`, not `#[cfg(test)]`) that a path of `pub mod`s or `pub
//! use`s reaches from the crate root, with their inherent `pub` methods,
//! trait methods and enum variants.
//!
//! The module tree and re-exports are read from the source text rather
//! than the syntax tree, because crates declare both inside macros
//! (`cfg_if!`, reqwest's `if_hyper!`) whose bodies tree-sitter does not
//! parse. Every `#[cfg]` is taken as enabled, as docs.rs's all-features
//! build does.

use super::space::{self, Module, Use};
use super::text::{self, Cut, Style};
use super::{Index, Item, Package, Source};
use jouet_graph::extract::Node;
use regex::Regex;
use std::collections::HashMap;
use std::path::Path;
use std::sync::OnceLock;

/// Directories of a crate that are not part of its library.
const SKIP_DIRS: [&str; 6] = [
    "target",
    "tests",
    "benches",
    "examples",
    "fuzz",
    "node_modules",
];

const TYPE_KINDS: [&str; 5] = ["struct", "enum", "trait", "union", "type_alias"];
const TOP_KINDS: [&str; 8] = [
    "struct",
    "enum",
    "trait",
    "union",
    "type_alias",
    "function",
    "constant",
    "variable",
];
const MEMBER_KINDS: [&str; 4] = ["method", "enum_member", "field", "property"];

pub(super) fn build(pkg: &Package) -> Result<Index, String> {
    let root = &pkg.api_root;
    let (lib_name, lib_path) = lib_target(root, &pkg.name);
    let rels = super::walk(
        root,
        |name| name.ends_with(".rs"),
        |name| !SKIP_DIRS.contains(&name),
    );
    if !rels.contains(&lib_path) {
        return Err(format!(
            "{} {} has no library target ({} is missing)",
            pkg.name, pkg.version, lib_path
        ));
    }
    let mut sources = super::parse(root, rels);
    for source in &mut sources {
        let more = macro_items(source);
        source.nodes.extend(more);
    }
    let mut builder = Builder {
        by_rel: sources
            .iter()
            .enumerate()
            .map(|(i, s)| (s.rel.clone(), i))
            .collect(),
        sources: &sources,
        modules: vec![Module::new(vec![lib_name.clone()], true)],
        items: Vec::new(),
        pending: Vec::new(),
        impls: Vec::new(),
        loaded: Vec::new(),
    };
    let lib = builder.by_rel[&lib_path];
    let child_dir = parent_dir(&lib_path);
    builder.load(lib, 0, &child_dir);
    builder.attach_members();
    let Builder {
        modules, mut items, ..
    } = builder;
    space::assign(&modules, &[0], &mut items, "::");
    super::finish(&mut items, "::");
    super::sort_paths(&mut items);
    Ok(Index {
        format: 0,
        doc: text::inner_doc(&sources[lib].text),
        items,
    })
}

/// The library's name and its root file, relative: `[lib]` in Cargo.toml,
/// else the package's name and `src/lib.rs`.
fn lib_target(root: &Path, package: &str) -> (String, String) {
    let manifest = std::fs::read_to_string(root.join("Cargo.toml"))
        .ok()
        .and_then(|t| toml::from_str::<toml::Value>(&t).ok());
    let lib = manifest.as_ref().and_then(|m| m.get("lib"));
    let field = |key: &str| lib.and_then(|l| l.get(key)).and_then(toml::Value::as_str);
    let name = field("name").map_or_else(|| package.replace('-', "_"), str::to_string);
    let path = field("path").map_or_else(
        || "src/lib.rs".to_string(),
        |p| p.trim_start_matches("./").to_string(),
    );
    (name, path)
}

fn parent_dir(rel: &str) -> String {
    rel.rsplit_once('/')
        .map_or_else(String::new, |(d, _)| d.to_string())
}

fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_string()
    } else {
        format!("{dir}/{name}")
    }
}

/// A `mod` declaration.
struct ModDecl {
    /// 0-based.
    line: usize,
    name: String,
    public: bool,
    /// An inline module's body, as 0-based lines.
    body: Option<(usize, usize)>,
    hidden: bool,
    test: bool,
    path_attr: Option<String>,
    doc: Option<String>,
}

/// The `mod` declarations and `pub use` statements of one file.
struct Scan {
    mods: Vec<ModDecl>,
    /// (0-based line, the statement).
    uses: Vec<(usize, String)>,
    /// Blocks under `#[cfg(test)]` (a `mod tests { }`, an `impl`), as
    /// 0-based lines: nothing in them is API.
    tests: Vec<(usize, usize)>,
    /// Trait impls: (0-based line, trait, the type's name).
    impls: Vec<(usize, String, String)>,
}

impl Scan {
    fn in_tests(&self, line: usize) -> bool {
        self.tests.iter().any(|&(a, b)| a <= line && line <= b)
    }
}

fn scan(lines: &[&str]) -> Scan {
    static MOD: OnceLock<Regex> = OnceLock::new();
    static USE: OnceLock<Regex> = OnceLock::new();
    static PATH: OnceLock<Regex> = OnceLock::new();
    let module = MOD.get_or_init(|| {
        Regex::new(r"^\s*(pub(?:\s*\([^)]*\))?\s+)?mod\s+(?:r#)?([A-Za-z_]\w*)\s*(;|\{)").unwrap()
    });
    let pub_use = USE.get_or_init(|| Regex::new(r"^\s*pub\s+use\s").unwrap());
    let path = PATH.get_or_init(|| Regex::new(r#"#\[path\s*=\s*"([^"]+)"\]"#).unwrap());
    let mut out = Scan {
        mods: Vec::new(),
        uses: Vec::new(),
        tests: Vec::new(),
        impls: Vec::new(),
    };
    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("#[cfg(test)]")
            && let Some(item) = (i + 1..lines.len().min(i + 8)).find(|&j| {
                let next = lines[j].trim_start();
                !next.starts_with("#[") && !next.starts_with("//") && !next.is_empty()
            })
        {
            let code = lines[item].split("//").next().unwrap_or("");
            let opens = code.find('{');
            if opens.is_some() && code.find(';').is_none_or(|semi| Some(semi) > opens) {
                out.tests.push((i, block_end(lines, item)));
            }
        }
        let unsafe_free = trimmed.strip_prefix("unsafe ").unwrap_or(trimmed);
        if (unsafe_free.starts_with("impl<") || unsafe_free.starts_with("impl "))
            && let Some((name, ty)) = trait_impl(&text::header(
                lines,
                i + 1,
                lines.len().min(i + 12),
                Cut::Body,
            ))
        {
            out.impls.push((i, name, ty));
        }
        if let Some(c) = module.captures(line) {
            let (doc, attrs) = text::doc_above(lines, i + 1, Style::Rust);
            let attrs = attrs.join(" ");
            out.mods.push(ModDecl {
                line: i,
                name: c[2].to_string(),
                public: c.get(1).is_some_and(|v| v.as_str().trim() == "pub"),
                body: (&c[3] == "{").then(|| (i, block_end(lines, i))),
                hidden: attrs.contains("doc(hidden)"),
                test: attrs.contains("cfg(test)"),
                path_attr: path.captures(&attrs).map(|p| p[1].to_string()),
                doc,
            });
        } else if pub_use.is_match(line) {
            let (_, attrs) = text::doc_above(lines, i + 1, Style::Rust);
            if attrs.iter().any(|a| a.contains("doc(hidden)")) {
                continue;
            }
            let mut statement = String::new();
            for next in &lines[i..lines.len().min(i + 60)] {
                let code = next.split("//").next().unwrap_or("");
                statement.push_str(code);
                statement.push(' ');
                if code.contains(';') {
                    break;
                }
            }
            out.uses.push((i, statement));
        }
    }
    out
}

/// The line (0-based) holding the `}` that closes the block opened on
/// line `start`.
fn block_end(lines: &[&str], start: usize) -> usize {
    let mut depth = 0i32;
    let mut opened = false;
    for (i, line) in lines.iter().enumerate().skip(start) {
        let code = line.split("//").next().unwrap_or("");
        let mut quoted = false;
        for c in code.chars() {
            match c {
                '"' => quoted = !quoted,
                '{' if !quoted => {
                    depth += 1;
                    opened = true;
                }
                '}' if !quoted => depth -= 1,
                _ => {}
            }
        }
        if opened && depth <= 0 {
            return i;
        }
    }
    lines.len().saturating_sub(1)
}

/// One `use` tree's leaves: a path and its alias, or a glob (`None`).
fn use_tree(statement: &str) -> Vec<(Vec<String>, Option<String>)> {
    let body = statement.trim();
    let body = body.strip_prefix("pub").unwrap_or(body).trim_start();
    let body = body.strip_prefix("use").unwrap_or(body);
    let body = body.split(';').next().unwrap_or("");
    let mut tokens: Vec<String> = Vec::new();
    let mut word = String::new();
    let mut chars = body.chars().peekable();
    while let Some(c) = chars.next() {
        if c.is_alphanumeric() || c == '_' || c == '#' {
            word.push(c);
            continue;
        }
        if !word.is_empty() {
            tokens.push(std::mem::take(&mut word));
        }
        match c {
            ':' if chars.peek() == Some(&':') => {
                chars.next();
                tokens.push("::".into());
            }
            '{' | '}' | ',' | '*' => tokens.push(c.to_string()),
            _ => {}
        }
    }
    if !word.is_empty() {
        tokens.push(word);
    }
    let mut at = 0;
    let mut out = Vec::new();
    tree(&tokens, &mut at, &[], &mut out);
    out
}

fn tree(
    tokens: &[String],
    at: &mut usize,
    prefix: &[String],
    out: &mut Vec<(Vec<String>, Option<String>)>,
) {
    let mut path: Vec<String> = prefix.to_vec();
    if tokens.get(*at).is_some_and(|t| t == "::") {
        *at += 1;
    }
    while let Some(token) = tokens.get(*at) {
        match token.as_str() {
            "::" => *at += 1,
            "{" => {
                *at += 1;
                while let Some(t) = tokens.get(*at) {
                    match t.as_str() {
                        "}" => {
                            *at += 1;
                            return;
                        }
                        "," => *at += 1,
                        _ => tree(tokens, at, &path, out),
                    }
                }
                return;
            }
            "*" => {
                *at += 1;
                out.push((path, None));
                return;
            }
            "," | "}" => break,
            "as" => {
                *at += 1;
                let alias = tokens.get(*at).cloned();
                *at += 1;
                if let Some(alias) = alias {
                    if path.last().is_some_and(|l| l == "self") {
                        path.pop();
                    }
                    out.push((path, Some(alias)));
                }
                return;
            }
            word => {
                path.push(word.trim_start_matches("r#").to_string());
                *at += 1;
            }
        }
    }
    let named_self = path.last().is_some_and(|l| l == "self");
    if named_self {
        path.pop();
    }
    if !path.is_empty() && (named_self || path.len() > prefix.len()) {
        let alias = path.last().cloned();
        out.push((path, alias));
    }
}

/// Items declared inside item-level macro invocations (`cfg_rt! { pub fn
/// spawn() {} }`), whose bodies tree-sitter leaves unparsed. Each layer of
/// bodies is extracted again with every other line blanked, so the line
/// numbers hold; a body inside a function is code, not items, and is left
/// alone.
fn macro_items(source: &Source) -> Vec<Node> {
    static OPEN: OnceLock<Regex> = OnceLock::new();
    let open = OPEN.get_or_init(|| {
        Regex::new(r"^\s*(?:[A-Za-z_][\w:]*::)?([A-Za-z_]\w*)!\s*\{\s*(?://.*)?$").unwrap()
    });
    let original: Vec<&str> = source.lines();
    let mut current: Vec<String> = original.iter().map(|l| (*l).to_string()).collect();
    let mut code: Vec<(usize, usize)> = spans(&source.nodes);
    let mut out = Vec::new();
    for _ in 0..3 {
        let lines: Vec<&str> = current.iter().map(String::as_str).collect();
        let mut bodies: Vec<(usize, usize)> = Vec::new();
        let mut i = 0;
        while i < lines.len() {
            let inside_code = code.iter().any(|&(a, b)| a < i && i <= b);
            match open.captures(lines[i]) {
                Some(c) if !inside_code => {
                    let end = block_end(&lines, i);
                    if &c[1] != "macro_rules" && end > i + 1 {
                        bodies.push((i, end));
                    }
                    i = end + 1;
                }
                _ => i += 1,
            }
        }
        if bodies.is_empty() {
            break;
        }
        let masked: Vec<String> = (0..original.len())
            .map(|i| {
                if bodies.iter().any(|&(a, b)| a < i && i < b) {
                    current[i].clone()
                } else {
                    String::new()
                }
            })
            .collect();
        let Some(Ok(extraction)) = jouet_graph::extract::extract(&source.rel, &masked.join("\n"))
        else {
            break;
        };
        let found: Vec<Node> = extraction
            .nodes
            .into_iter()
            .filter(|n| !matches!(n.kind, "file" | "import"))
            .collect();
        code.extend(spans(&found));
        out.extend(found);
        current = masked;
    }
    out
}

/// The 0-based line spans of functions and methods: their bodies are code.
fn spans(nodes: &[Node]) -> Vec<(usize, usize)> {
    nodes
        .iter()
        .filter(|n| matches!(n.kind, "function" | "method"))
        .map(|n| {
            (
                n.start_line.saturating_sub(1) as usize,
                n.end_line.saturating_sub(1) as usize,
            )
        })
        .collect()
}

/// A member waiting for its owner: (item, owner's name, module, source).
type Pending = (usize, String, usize, usize);

/// A module within one file: an inline module's body (0-based lines; none
/// for the file's own module), the module, and its children's directory.
type Scope = (Option<(usize, usize)>, usize, String);

struct Builder<'a> {
    sources: &'a [Source],
    by_rel: HashMap<String, usize>,
    modules: Vec<Module>,
    items: Vec<Item>,
    pending: Vec<Pending>,
    /// Trait impls waiting for their type: (trait, type's name, module,
    /// source).
    impls: Vec<(String, String, usize, usize)>,
    /// Sources already loaded as a module, against a file declared twice.
    loaded: Vec<usize>,
}

impl Builder<'_> {
    /// Loads source `file` as module `module`, whose child modules' files
    /// are under `child_dir`.
    fn load(&mut self, file: usize, module: usize, child_dir: &str) {
        if self.loaded.contains(&file) || self.modules[module].path.len() > 12 {
            return;
        }
        self.loaded.push(file);
        let source = &self.sources[file];
        let lines = source.lines();
        let scan = scan(&lines);
        // The file's module and its inline modules, by body: (span, module,
        // its children's directory).
        let mut scopes: Vec<Scope> = vec![(None, module, child_dir.to_string())];
        let mut files: Vec<(usize, usize, String)> = Vec::new();
        for decl in &scan.mods {
            if decl.test || scan.in_tests(decl.line) {
                continue;
            }
            let (_, parent, scope_dir) = innermost(&scopes, decl.line).clone();
            // The same module declared again under another `#[cfg]`: one
            // module, as public as its most public declaration.
            if let Some(&existing) = self.modules[parent].children.get(&decl.name) {
                self.modules[existing].public |= decl.public && !decl.hidden;
                if let Some(span) = decl.body {
                    scopes.push((Some(span), existing, join(&scope_dir, &decl.name)));
                }
                continue;
            }
            let mut path = self.modules[parent].path.clone();
            path.push(decl.name.clone());
            let id = self.modules.len();
            let mut m = Module::new(path.clone(), decl.public && !decl.hidden);
            let end = decl.body.map_or(decl.line, |(_, e)| e);
            let mut item = Item::new(
                "module",
                &decl.name,
                &source.rel,
                (decl.line as u32 + 1, end as u32 + 1),
            );
            item.signature = format!("{}mod {}", if decl.public { "pub " } else { "" }, decl.name);
            item.doc.clone_from(&decl.doc);
            item.home = path.join("::");
            m.item = Some(self.items.len());
            self.items.push(item);
            self.modules.push(m);
            self.modules[parent].children.insert(decl.name.clone(), id);
            let dir = join(&scope_dir, &decl.name);
            match decl.body {
                Some(span) => scopes.push((Some(span), id, dir)),
                None => {
                    let candidates = match &decl.path_attr {
                        Some(p) => vec![join(&parent_dir(&source.rel), p)],
                        None => vec![format!("{dir}.rs"), format!("{dir}/mod.rs")],
                    };
                    if let Some(&f) = candidates.iter().find_map(|c| self.by_rel.get(c)) {
                        files.push((f, id, dir));
                    }
                }
            }
        }
        self.load_items(file, &scopes, &scan);
        for (line, name, ty) in &scan.impls {
            if !scan.in_tests(*line) {
                let (_, m, _) = innermost(&scopes, *line).clone();
                self.impls.push((name.clone(), ty.clone(), m, file));
            }
        }
        for (line, statement) in &scan.uses {
            if scan.in_tests(*line) {
                continue;
            }
            let (_, m, _) = innermost(&scopes, *line).clone();
            for (path, alias) in use_tree(statement) {
                if let Some(u) = self.resolve_use(m, &path, alias, &source.rel, *line) {
                    self.modules[m].uses.push(u);
                }
            }
        }
        for (f, id, dir) in files {
            if self.modules[id]
                .item
                .is_some_and(|i| self.items[i].doc.is_none())
            {
                let doc = text::inner_doc(&self.sources[f].text);
                if let Some(i) = self.modules[id].item {
                    self.items[i].doc = doc;
                }
            }
            self.load(f, id, &dir);
        }
    }

    /// The items the extractor found in `file`, each in the module whose
    /// body holds it.
    fn load_items(&mut self, file: usize, scopes: &[Scope], scan: &Scan) {
        let source = &self.sources[file];
        let lines = source.lines();
        let inline_names: Vec<String> = scopes
            .iter()
            .skip(1)
            .filter_map(|(_, m, _)| self.modules[*m].path.last().cloned())
            .collect();
        for node in &source.nodes {
            let top = TOP_KINDS.contains(&node.kind);
            let member = MEMBER_KINDS.contains(&node.kind);
            if !top && !member {
                continue;
            }
            let prefix: Vec<&str> = node
                .qualified_name
                .split("::")
                .filter(|s| !inline_names.iter().any(|n| n == s))
                .collect();
            let owner = match (top, prefix.len()) {
                (true, 1) => None,
                (false, 2) => Some(prefix[0].to_string()),
                // Inside a function, or something jouet does not place.
                _ => continue,
            };
            let start = node.start_line as usize;
            let (doc, attrs) = text::doc_above(&lines, start, Style::Rust);
            let attrs = attrs.join(" ");
            if attrs.contains("cfg(test)") || scan.in_tests(start.saturating_sub(1)) {
                continue;
            }
            let (_, m, _) = innermost(scopes, start.saturating_sub(1)).clone();
            let cut = if member && node.kind != "method" {
                Cut::Short
            } else {
                Cut::Body
            };
            let mut item = Item::new(
                node.kind,
                &node.name,
                &source.rel,
                (node.start_line, node.end_line),
            );
            item.signature = text::header(&lines, start, node.end_line as usize, cut)
                .trim_end_matches(',')
                .to_string();
            // The extractor's docstring, when there is a doc comment: a plain
            // `// SAFETY:` or licence comment above an item is not its doc.
            item.doc = doc
                .map(|own| node.docstring.clone().unwrap_or(own))
                .map(|d| visible_doc(&d));
            if TYPE_KINDS.contains(&node.kind) {
                item.implements = derives(&attrs);
            }
            let hidden = attrs.contains("doc(hidden)");
            item.public = !hidden && is_pub(&item.signature);
            let mut home = self.modules[m].path.clone();
            if let Some(owner) = &owner {
                home.push(owner.clone());
            }
            home.push(node.name.clone());
            item.home = home.join("::");
            let idx = self.items.len();
            self.items.push(item);
            match owner {
                None => {
                    let module = &mut self.modules[m];
                    module.defs.entry(node.name.clone()).or_insert(idx);
                    if self.items[idx].public {
                        module.exports.push((node.name.clone(), idx));
                    }
                }
                Some(owner) => self.pending.push((idx, owner, m, file)),
            }
        }
    }

    /// A `pub use` leaf in module `m`, as a [`Use`].
    fn resolve_use(
        &self,
        m: usize,
        path: &[String],
        alias: Option<String>,
        file: &str,
        line: usize,
    ) -> Option<Use> {
        let first = path.first()?;
        let here = &self.modules[m];
        let mut abs: Vec<String> = match first.as_str() {
            "crate" => self.modules[0].path.clone(),
            "self" => here.path.clone(),
            "super" => {
                let ups = path.iter().take_while(|s| *s == "super").count();
                here.path[..here.path.len().saturating_sub(ups).max(1)].to_vec()
            }
            name if here.children.contains_key(name) => here.path.clone(),
            // 2015 edition: paths are from the crate root.
            name if self.modules[0].children.contains_key(name) => self.modules[0].path.clone(),
            _ => {
                let text = format!(
                    "pub use {}{};",
                    path.join("::"),
                    match &alias {
                        Some(a) if path.last() != Some(a) => format!(" as {a}"),
                        _ => String::new(),
                    }
                );
                return Some(Use::External {
                    alias: alias?,
                    text,
                    file: file.to_string(),
                    line: line as u32 + 1,
                });
            }
        };
        let skip = match first.as_str() {
            "crate" | "self" => 1,
            "super" => path.iter().take_while(|s| *s == "super").count(),
            _ => 0,
        };
        abs.extend(path[skip..].iter().cloned());
        match alias {
            None => Some(Use::Glob { module: abs }),
            Some(alias) => {
                let name = abs.pop()?;
                Some(Use::Name {
                    module: abs,
                    name,
                    alias,
                })
            }
        }
    }

    /// Gives each method, field and variant its owner: the type of that name
    /// in the same module, else in the same file, else the only one in the
    /// crate.
    fn attach_members(&mut self) {
        let types: Vec<usize> = (0..self.items.len())
            .filter(|&i| {
                self.items[i].owner.is_none() && TYPE_KINDS.contains(&self.items[i].kind.as_str())
            })
            .collect();
        for (idx, owner, m, file) in std::mem::take(&mut self.pending) {
            let Some(found) = self.owner(&types, &owner, m, file) else {
                continue;
            };
            let owner_kind = self.items[found].kind.clone();
            let item = &mut self.items[idx];
            item.owner = Some(found);
            // A trait's methods and an enum's variants are as public as it is.
            if owner_kind == "trait" || item.kind == "enum_member" {
                item.public = true;
            }
        }
        for (name, ty, m, file) in std::mem::take(&mut self.impls) {
            if let Some(found) = self.owner(&types, &ty, m, file)
                && !self.items[found].implements.contains(&name)
            {
                self.items[found].implements.push(name);
            }
        }
    }

    /// The type named `name` that a method or impl in module `m` of source
    /// `file` belongs to: the one in that module, else in that file, else
    /// the only one in the crate.
    fn owner(&self, types: &[usize], name: &str, m: usize, file: usize) -> Option<usize> {
        let rel = &self.sources[file].rel;
        let named: Vec<usize> = types
            .iter()
            .copied()
            .filter(|&t| self.items[t].name == name)
            .collect();
        self.modules[m]
            .defs
            .get(name)
            .copied()
            .filter(|t| named.contains(t))
            .or_else(|| named.iter().copied().find(|&t| &self.items[t].file == rel))
            .or_else(|| (named.len() == 1).then(|| named[0]))
    }
}

/// The traits `#[derive(..)]` attributes name, also inside `cfg_attr`.
fn derives(attrs: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (at, _) in attrs.match_indices("derive(") {
        let list = &attrs[at + "derive(".len()..];
        let list = &list[..list.find(')').unwrap_or(list.len())];
        for name in list.split(',') {
            let name = name.trim().rsplit("::").next().unwrap_or("").to_string();
            if !name.is_empty() && !out.contains(&name) {
                out.push(name);
            }
        }
    }
    out
}

/// The trait and the type's name of an `impl Trait for Type` header:
/// `impl<T: Read> io::Read for Body<T>` is `("Read", "Body")`. `None` for
/// an inherent impl or a blanket one (`impl<T> Trait for T` names no type
/// of the crate, and its type is not found later).
fn trait_impl(header: &str) -> Option<(String, String)> {
    let text = header.split_whitespace().collect::<Vec<_>>().join(" ");
    let text = text.strip_prefix("unsafe ").unwrap_or(&text);
    let rest = text.strip_prefix("impl")?;
    let rest = skip_generics(rest.trim_start());
    let (name, ty) = split_top(rest, " for ")?;
    let ty = split_top(ty, " where ").map_or(ty, |(t, _)| t);
    let name = last_segment(name.trim());
    let ty = ty.trim().trim_start_matches('&').trim_start();
    // `&'a mut T`: past the lifetime, then `mut` or `dyn`.
    let ty = match ty.strip_prefix('\'') {
        Some(lifetime) => lifetime.split_once(' ').map_or("", |(_, t)| t),
        None => ty,
    };
    let ty = ty.trim_start_matches("mut ").trim_start_matches("dyn ");
    let ty = ty.split('<').next().unwrap_or(ty);
    let ty = ty.rsplit("::").next().unwrap_or(ty).trim();
    (!name.is_empty() && !ty.is_empty() && ty.chars().all(|c| c.is_alphanumeric() || c == '_'))
        .then(|| (name, ty.to_string()))
}

/// `text` after a leading `<...>`, brackets nested.
fn skip_generics(text: &str) -> &str {
    if !text.starts_with('<') {
        return text;
    }
    let mut depth = 0;
    let mut prev = ' ';
    for (at, c) in text.char_indices() {
        match c {
            '<' => depth += 1,
            '>' if prev != '-' => {
                depth -= 1;
                if depth == 0 {
                    return text[at + 1..].trim_start();
                }
            }
            _ => {}
        }
        prev = c;
    }
    ""
}

/// `text` split at the first `sep` outside `<...>` and `(...)`.
fn split_top<'a>(text: &'a str, sep: &str) -> Option<(&'a str, &'a str)> {
    let mut depth = 0i32;
    let mut prev = ' ';
    for (at, c) in text.char_indices() {
        match c {
            '<' | '(' | '[' => depth += 1,
            '>' if prev != '-' => depth -= 1,
            ')' | ']' => depth -= 1,
            _ if depth == 0 && text[at..].starts_with(sep) => {
                return Some((&text[..at], &text[at + sep.len()..]));
            }
            _ => {}
        }
        prev = c;
    }
    None
}

/// A path's last segment, with its generic arguments: `fmt::Display` is
/// `Display`, `std::convert::From<Config>` is `From<Config>`.
fn last_segment(path: &str) -> String {
    let head = path.find('<').map_or(path, |at| &path[..at]);
    let start = head.rfind("::").map_or(0, |at| at + 2);
    path[start..].to_string()
}

/// The scope whose body holds line `line` (0-based), innermost first.
fn innermost(scopes: &[Scope], line: usize) -> &Scope {
    scopes
        .iter()
        .rev()
        .find(|(span, _, _)| span.is_none_or(|(a, b)| a < line && line <= b))
        .unwrap_or(&scopes[0])
}

/// A doc without the lines rustdoc hides from a code example (`# use x;`).
fn visible_doc(doc: &str) -> String {
    let mut fenced = false;
    let mut rust = false;
    let mut out: Vec<&str> = Vec::new();
    for line in doc.lines() {
        let trimmed = line.trim_start();
        if let Some(info) = trimmed.strip_prefix("```") {
            if !fenced {
                let lang = info.trim();
                rust = lang.is_empty()
                    || lang.split(',').all(|t| {
                        matches!(
                            t.trim(),
                            "rust"
                                | "no_run"
                                | "ignore"
                                | "should_panic"
                                | "compile_fail"
                                | "edition2018"
                                | "edition2021"
                                | "edition2024"
                        )
                    });
            }
            fenced = !fenced;
        } else if fenced && rust && (trimmed == "#" || trimmed.starts_with("# ")) {
            continue;
        }
        out.push(line);
    }
    out.join("\n")
}

/// Declared plain `pub`, not `pub(crate)` or `pub(super)`.
fn is_pub(signature: &str) -> bool {
    signature
        .strip_prefix("pub")
        .is_some_and(|rest| rest.starts_with(char::is_whitespace))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leaves(statement: &str) -> Vec<String> {
        use_tree(statement)
            .into_iter()
            .map(|(path, alias)| match alias {
                Some(a) => format!("{} as {a}", path.join("::")),
                None => format!("{}::*", path.join("::")),
            })
            .collect()
    }

    #[test]
    fn use_trees_are_flattened() {
        assert_eq!(
            leaves("pub use self::client::{Client, ClientBuilder};"),
            [
                "self::client::Client as Client",
                "self::client::ClientBuilder as ClientBuilder"
            ]
        );
        assert_eq!(
            leaves("pub use http::{self, Method as M, header::*};"),
            ["http as http", "http::Method as M", "http::header::*"]
        );
        assert_eq!(leaves("pub use ::url::Url;"), ["url::Url as Url"]);
        assert_eq!(leaves("pub use crate::a::*;"), ["crate::a::*"]);
    }

    #[test]
    fn rustdoc_hidden_lines_are_left_out() {
        assert_eq!(
            visible_doc(
                "Run it.\n\n```\n# use x::y;\nlet a = y();\n# Ok::<_, E>(())\n```\n# Errors\n```text\n# kept\n```"
            ),
            "Run it.\n\n```\nlet a = y();\n```\n# Errors\n```text\n# kept\n```"
        );
    }

    #[test]
    fn test_blocks_are_not_api() {
        let src = "pub fn a() {}\n#[cfg(test)]\nimpl A {\n    pub fn b() {}\n}\n#[cfg(test)]\nmod tests;\n";
        let lines: Vec<&str> = src.lines().collect();
        let found = scan(&lines);
        assert_eq!(found.tests, [(1, 4)]);
        assert!(found.in_tests(3));
        assert!(!found.in_tests(0));
        assert!(found.mods[0].test);
    }

    #[test]
    fn items_inside_item_level_macros_are_found() {
        let text = "cfg_rt! {\n    /// Spawns it.\n    pub fn spawn() {}\n\n    cfg_fs! {\n        pub struct File;\n    }\n}\n\npub fn run() {\n    select! {\n        fn not_an_item() {}\n    }\n}\nmacro_rules! m {\n    ($i:item) => { $i };\n}\n";
        let source = Source {
            rel: "src/lib.rs".into(),
            text: text.into(),
            nodes: jouet_graph::extract::extract("src/lib.rs", text)
                .unwrap()
                .unwrap()
                .nodes,
        };
        let found: Vec<(String, u32)> = macro_items(&source)
            .iter()
            .map(|n| (n.name.clone(), n.start_line))
            .collect();
        assert_eq!(found, [("spawn".to_string(), 3), ("File".to_string(), 6)]);
    }

    #[test]
    fn trait_impls_name_the_trait_and_the_type() {
        let pair = |h: &str| trait_impl(h).map(|(t, ty)| format!("{t} for {ty}"));
        assert_eq!(
            pair("impl Clone for Agent").as_deref(),
            Some("Clone for Agent")
        );
        assert_eq!(
            pair("impl<T: Read> io::Read for Body<T>").as_deref(),
            Some("Read for Body")
        );
        assert_eq!(
            pair("impl<'a, F: Fn() -> u8> From<Vec<F>> for &'a mut Pool<'a>\nwhere\n    F: Send")
                .as_deref(),
            Some("From<Vec<F>> for Pool")
        );
        assert_eq!(
            pair("unsafe impl Send for Agent").as_deref(),
            Some("Send for Agent")
        );
        assert_eq!(pair("impl Agent"), None);
        assert_eq!(pair("impl<T> Agent<T>"), None);
        assert_eq!(pair("impl<T> Show for [T]"), None);
        assert_eq!(
            derives(
                "#[derive(Clone, Debug)] #[cfg_attr(feature = \"serde\", derive(serde::Serialize))]"
            ),
            ["Clone", "Debug", "Serialize"]
        );
    }

    #[test]
    fn plain_pub_is_public() {
        assert!(is_pub("pub fn get()"));
        assert!(is_pub("pub\nstruct A"));
        assert!(!is_pub("pub(crate) fn get()"));
        assert!(!is_pub("fn get()"));
    }
}

//! The definitions in a source file, from the definition patterns of each
//! grammar's own tags query (`TAGS_QUERY`; TypeScript's together with
//! JavaScript's, as tree-sitter's CLI combines them): to group search
//! matches, and to read and edit code by symbol.
//!
//! Built without the `grammars` feature, no language is supported and every
//! file has no symbols, so search falls back to plain matches.

use serde::{Deserialize, Serialize};
use std::path::Path;

/// A definition: a function, method, class, module and so on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Def {
    pub name: String,
    /// What the grammar's query calls it: `function`, `method`, `class`,
    /// `interface`, `module`, `macro`, `type`, `constant` ...
    pub kind: String,
    /// The 1-based line of its name.
    pub line: u32,
    /// The 1-based first and last lines of the whole definition.
    pub start: u32,
    pub end: u32,
    /// The line of its name, trailing whitespace removed and cut to
    /// [`TEXT_MAX`] bytes.
    pub text: String,
}

/// The innermost of `defs` containing a 1-based line.
pub fn enclosing(defs: &[Def], line: u32) -> Option<&Def> {
    defs.iter()
        .filter(|d| d.start <= line && line <= d.end)
        .min_by_key(|d| (d.end - d.start, std::cmp::Reverse(d.start)))
}

/// How much of a definition's line is kept.
pub const TEXT_MAX: usize = 160;

/// Whether jouet can read symbols from this file, judged by its name.
pub fn supported(path: &Path) -> bool {
    imp::language(path).is_some()
}

/// The file's definitions in source order, or `None` when its language is
/// not supported or it cannot be parsed.
pub fn definitions(path: &Path, source: &[u8]) -> Option<Vec<Def>> {
    imp::definitions(path, source)
}

/// A definition with where it sits: its byte span and the names it is
/// nested in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Located {
    pub def: Def,
    /// The byte range of the whole definition.
    pub start_byte: usize,
    pub end_byte: usize,
    /// The names it is nested in, outermost first: a class, a module, or
    /// the type of a Rust `impl` block.
    pub scope: Vec<String>,
    /// How the language joins a scope to a name: `::` or `.`.
    pub separator: &'static str,
}

impl Located {
    /// The name with its scope, as a selector names it: `Vfox::download`.
    pub fn qualified(&self) -> String {
        let mut parts = self.scope.clone();
        parts.push(self.def.name.clone());
        parts.join(self.separator)
    }
}

/// Every definition in the file with its span and scope, in source order,
/// or `None` when the language is not supported or it cannot be parsed.
pub fn outline(path: &Path, source: &[u8]) -> Option<Vec<Located>> {
    imp::outline(path, source)
}

/// Why a selector names no single definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectError {
    /// jouet cannot read definitions from this file.
    Unsupported,
    /// The selector is malformed, such as an unknown kind.
    Invalid(String),
    /// Nothing matches.
    NotFound,
}

/// A selector: `NAME`, `KIND NAME`, or either with a scope, such as `fn
/// download`, `Vfox::download` or `class Shop.Cart`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selector {
    kinds: Option<&'static [&'static str]>,
    path: Vec<String>,
}

impl Selector {
    pub fn parse(text: &str) -> Result<Self, SelectError> {
        let text = text.trim();
        let (kinds, name) = match text.split_once(char::is_whitespace) {
            Some((kind, name)) => (Some(kinds(kind)?), name.trim()),
            None => (None, text),
        };
        let path: Vec<String> = name
            .split("::")
            .flat_map(|part| part.split('.'))
            .map(str::to_string)
            .collect();
        if path
            .iter()
            .any(|p| p.is_empty() || p.contains(char::is_whitespace))
        {
            return Err(SelectError::Invalid(format!(
                "`{text}` is not a symbol: give a name, such as `download`, `fn download` or `Vfox::download`"
            )));
        }
        Ok(Self { kinds, path })
    }

    fn matches(&self, found: &Located) -> bool {
        let (name, scope) = self
            .path
            .split_last()
            .expect("parse keeps one part at least");
        found.def.name == *name
            && found.scope.ends_with(scope)
            && self
                .kinds
                .is_none_or(|kinds| kinds.contains(&found.def.kind.as_str()))
    }
}

/// The kinds a selector's kind word stands for.
fn kinds(word: &str) -> Result<&'static [&'static str], SelectError> {
    Ok(match word {
        "fn" | "func" | "function" | "def" | "method" => &["function", "method"],
        "struct" | "class" | "enum" | "union" => &["class", "type"],
        "type" => &["type", "class", "interface"],
        "trait" | "interface" | "protocol" => &["interface"],
        "mod" | "module" | "namespace" => &["module"],
        "macro" => &["macro"],
        "const" | "constant" => &["constant"],
        other => {
            return Err(SelectError::Invalid(format!(
                "unknown kind `{other}`: use fn, struct, class, enum, type, trait, interface, mod, module, macro or const"
            )));
        }
    })
}

/// The definitions a selector names.
pub fn select(
    path: &Path,
    source: &[u8],
    selector: &Selector,
) -> Result<Vec<Located>, SelectError> {
    let all = outline(path, source).ok_or(SelectError::Unsupported)?;
    let found: Vec<Located> = all.into_iter().filter(|l| selector.matches(l)).collect();
    if found.is_empty() {
        return Err(SelectError::NotFound);
    }
    Ok(found)
}

/// How many syntax errors tree-sitter finds in the file, or `None` when the
/// language is not supported.
pub fn syntax_errors(path: &Path, source: &[u8]) -> Option<usize> {
    imp::syntax_errors(path, source)
}

/// The names of the languages this build supports.
pub fn languages() -> &'static [&'static str] {
    imp::LANGUAGES
}

/// A hash of every tags query this build runs, so symbols cached by an
/// older build are read again.
pub(crate) fn queries_hash() -> u64 {
    imp::queries()
        .iter()
        .fold(0xcbf2_9ce4_8422_2325, |hash, query| {
            fnv(hash, query.as_bytes())
        })
}

/// FNV-1a: stable across builds and platforms, unlike std's hasher.
pub(crate) fn fnv(mut hash: u64, bytes: &[u8]) -> u64 {
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

#[cfg(feature = "grammars")]
/// The 1-based line holding byte `offset`, given the offsets each line
/// starts at.
fn line_of(starts: &[usize], offset: usize) -> u32 {
    let index = starts.partition_point(|&s| s <= offset).max(1) - 1;
    u32::try_from(index + 1).unwrap_or(u32::MAX)
}

#[cfg(feature = "grammars")]
/// The byte offset each line starts at.
fn line_starts(source: &[u8]) -> Vec<usize> {
    std::iter::once(0)
        .chain(memchr::memchr_iter(b'\n', source).map(|i| i + 1))
        .collect()
}

#[cfg(feature = "grammars")]
/// A line's text for display: trailing whitespace removed, cut on a
/// character boundary.
fn line_text(source: &[u8], starts: &[usize], line: u32) -> String {
    let index = line.saturating_sub(1) as usize;
    let Some(&start) = starts.get(index) else {
        return String::new();
    };
    let end = starts.get(index + 1).copied().unwrap_or(source.len());
    let text = String::from_utf8_lossy(&source[start..end]);
    let text = text.trim_end();
    if text.len() <= TEXT_MAX {
        return text.to_string();
    }
    let mut cut = TEXT_MAX;
    while !text.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}...", &text[..cut])
}

#[cfg(feature = "grammars")]
mod imp {
    use super::{Def, Located, line_of, line_starts, line_text};
    use std::cell::RefCell;
    use std::collections::BTreeMap;
    use std::path::Path;
    use std::sync::OnceLock;
    use tree_sitter::{Node, Parser, Query, QueryCursor, StreamingIterator, Tree};

    pub(super) const LANGUAGES: &[&str] = &[
        "c",
        "cpp",
        "go",
        "java",
        "javascript",
        "python",
        "ruby",
        "rust",
        "typescript",
    ];

    #[derive(Clone, Copy)]
    pub(super) enum Lang {
        C,
        Cpp,
        Go,
        Java,
        JavaScript,
        Python,
        Ruby,
        Rust,
        TypeScript,
        Tsx,
    }

    pub(super) fn language(path: &Path) -> Option<Lang> {
        let ext = path.extension()?.to_str()?;
        Some(match ext {
            "c" | "h" => Lang::C,
            "cc" | "cpp" | "cxx" | "c++" | "hh" | "hpp" | "hxx" | "h++" => Lang::Cpp,
            "go" => Lang::Go,
            "java" => Lang::Java,
            "js" | "mjs" | "cjs" | "jsx" => Lang::JavaScript,
            "py" | "pyi" => Lang::Python,
            "rb" | "rake" | "gemspec" => Lang::Ruby,
            "rs" => Lang::Rust,
            "ts" | "mts" | "cts" => Lang::TypeScript,
            "tsx" => Lang::Tsx,
            _ => return None,
        })
    }

    /// A language's grammar, tags query and locals query.
    fn source(lang: Lang) -> (tree_sitter::Language, String) {
        let typescript = || {
            format!(
                "{}\n{}",
                tree_sitter_javascript::TAGS_QUERY,
                tree_sitter_typescript::TAGS_QUERY
            )
        };
        match lang {
            Lang::C => (
                tree_sitter_c::LANGUAGE.into(),
                tree_sitter_c::TAGS_QUERY.to_string(),
            ),
            Lang::Cpp => (
                tree_sitter_cpp::LANGUAGE.into(),
                tree_sitter_cpp::TAGS_QUERY.to_string(),
            ),
            Lang::Go => (
                tree_sitter_go::LANGUAGE.into(),
                tree_sitter_go::TAGS_QUERY.to_string(),
            ),
            Lang::Java => (
                tree_sitter_java::LANGUAGE.into(),
                tree_sitter_java::TAGS_QUERY.to_string(),
            ),
            Lang::JavaScript => (
                tree_sitter_javascript::LANGUAGE.into(),
                tree_sitter_javascript::TAGS_QUERY.to_string(),
            ),
            Lang::Python => (
                tree_sitter_python::LANGUAGE.into(),
                tree_sitter_python::TAGS_QUERY.to_string(),
            ),
            Lang::Ruby => (
                tree_sitter_ruby::LANGUAGE.into(),
                tree_sitter_ruby::TAGS_QUERY.to_string(),
            ),
            Lang::Rust => (
                tree_sitter_rust::LANGUAGE.into(),
                tree_sitter_rust::TAGS_QUERY.to_string(),
            ),
            Lang::TypeScript => (
                tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
                typescript(),
            ),
            Lang::Tsx => (tree_sitter_typescript::LANGUAGE_TSX.into(), typescript()),
        }
    }

    fn source_language(lang: Lang) -> tree_sitter::Language {
        match lang {
            Lang::C => tree_sitter_c::LANGUAGE.into(),
            Lang::Cpp => tree_sitter_cpp::LANGUAGE.into(),
            Lang::Go => tree_sitter_go::LANGUAGE.into(),
            Lang::Java => tree_sitter_java::LANGUAGE.into(),
            Lang::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
            Lang::Python => tree_sitter_python::LANGUAGE.into(),
            Lang::Ruby => tree_sitter_ruby::LANGUAGE.into(),
            Lang::Rust => tree_sitter_rust::LANGUAGE.into(),
            Lang::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            Lang::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
        }
    }

    /// The tags query with only its definition patterns enabled: what
    /// finding definitions runs, without the cost of every reference.
    fn definitions_query(lang: Lang) -> Option<&'static Query> {
        static QUERIES: [OnceLock<Option<Query>>; 10] = [const { OnceLock::new() }; 10];
        QUERIES[lang as usize]
            .get_or_init(|| {
                let (language, tags) = source(lang);
                let mut query = Query::new(&language, &tags).ok()?;
                for i in 0..query.pattern_count() {
                    let text =
                        &tags[query.start_byte_for_pattern(i)..query.end_byte_for_pattern(i)];
                    if !text.contains("@definition.") {
                        query.disable_pattern(i);
                    }
                }
                Some(query)
            })
            .as_ref()
    }

    pub(super) fn queries() -> Vec<&'static str> {
        vec![
            tree_sitter_c::TAGS_QUERY,
            tree_sitter_cpp::TAGS_QUERY,
            tree_sitter_go::TAGS_QUERY,
            tree_sitter_java::TAGS_QUERY,
            tree_sitter_javascript::TAGS_QUERY,
            tree_sitter_python::TAGS_QUERY,
            tree_sitter_ruby::TAGS_QUERY,
            tree_sitter_rust::TAGS_QUERY,
            tree_sitter_typescript::TAGS_QUERY,
        ]
    }

    thread_local! {
        static PARSER: RefCell<Parser> = RefCell::new(Parser::new());
    }

    fn parse_tree(lang: Lang, source: &[u8]) -> Option<Tree> {
        PARSER.with_borrow_mut(|parser| {
            parser.set_language(&source_language(lang)).ok()?;
            parser.parse(source, None)
        })
    }

    pub(super) fn definitions(path: &Path, source: &[u8]) -> Option<Vec<Def>> {
        outline(path, source).map(|all| all.into_iter().map(|l| l.def).collect())
    }

    pub(super) fn outline(path: &Path, source: &[u8]) -> Option<Vec<Located>> {
        let lang = language(path)?;
        let query = definitions_query(lang)?;
        let tree = parse_tree(lang, source)?;
        let names = query.capture_names();
        let starts = line_starts(source);
        let separator = match lang {
            Lang::Rust | Lang::Cpp => "::",
            _ => ".",
        };
        let mut cursor = QueryCursor::new();
        // One definition per node: the earliest pattern's, as
        // tree-sitter's tags tool picks it.
        let mut found: BTreeMap<(usize, usize), (usize, Located)> = BTreeMap::new();
        let mut matches = cursor.matches(query, tree.root_node(), source);
        while let Some(m) = matches.next() {
            let mut def = None;
            let mut name = None;
            for capture in m.captures() {
                let capture_name = names[capture.index as usize];
                if capture_name == "name" {
                    name = Some(capture.node);
                } else if let Some(kind) = capture_name.strip_prefix("definition.") {
                    def = Some((capture.node, kind));
                }
            }
            let (Some((node, kind)), Some(name)) = (def, name) else {
                continue;
            };
            let Ok(text) = name.utf8_text(source) else {
                continue;
            };
            let range = node.byte_range();
            let range = range.start.min(name.start_byte())..range.end.max(name.end_byte());
            let key = (range.start, range.end);
            if found
                .get(&key)
                .is_some_and(|(index, _)| *index <= m.pattern_index)
            {
                continue;
            }
            let line = line_of(&starts, name.start_byte());
            let end = range.end.saturating_sub(1).max(range.start);
            found.insert(
                key,
                (
                    m.pattern_index,
                    Located {
                        def: Def {
                            name: text.to_string(),
                            kind: kind.to_string(),
                            line,
                            start: line_of(&starts, range.start),
                            end: line_of(&starts, end),
                            text: line_text(source, &starts, line),
                        },
                        start_byte: range.start,
                        end_byte: range.end,
                        scope: scope(node, source),
                        separator,
                    },
                ),
            );
        }
        let mut all: Vec<Located> = found.into_values().map(|(_, l)| l).collect();
        all.sort_by_key(|l| (l.def.start, l.def.line));
        Some(all)
    }

    /// The names a definition is nested in, outermost first: each enclosing
    /// node's `name`, and the type of a Rust `impl` block.
    fn scope(node: Node, source: &[u8]) -> Vec<String> {
        let mut scope = Vec::new();
        let mut parent = node.parent();
        while let Some(p) = parent {
            let name = match p.kind() {
                "impl_item" => p.child_by_field_name("type"),
                _ => p.child_by_field_name("name"),
            };
            if let Some(text) = name.and_then(|n| n.utf8_text(source).ok()) {
                // `Vfox<T>` is `Vfox`; a qualified C++ name keeps its last part.
                let text = text.split('<').next().unwrap_or(text);
                let text = text.rsplit("::").next().unwrap_or(text).trim();
                if !text.is_empty() && !text.contains(char::is_whitespace) {
                    scope.push(text.to_string());
                }
            }
            parent = p.parent();
        }
        scope.reverse();
        // A Go method's scope is its receiver's type: `(s *Server) Serve`.
        if node.kind() == "method_declaration"
            && let Some(receiver) = node.child_by_field_name("receiver")
            && let Some(name) = first_of_kind(receiver, "type_identifier")
            && let Ok(text) = name.utf8_text(source)
        {
            scope.push(text.to_string());
        }
        scope
    }

    fn first_of_kind<'t>(node: Node<'t>, kind: &str) -> Option<Node<'t>> {
        if node.kind() == kind {
            return Some(node);
        }
        let mut cursor = node.walk();
        node.children(&mut cursor)
            .find_map(|child| first_of_kind(child, kind))
    }

    pub(super) fn syntax_errors(path: &Path, source: &[u8]) -> Option<usize> {
        let tree = parse_tree(language(path)?, source)?;
        let mut count = 0;
        let mut cursor = tree.walk();
        'walk: loop {
            let node = cursor.node();
            if node.is_error() || node.is_missing() {
                count += 1;
            }
            if node.has_error() && cursor.goto_first_child() {
                continue;
            }
            while !cursor.goto_next_sibling() {
                if !cursor.goto_parent() {
                    break 'walk;
                }
            }
        }
        Some(count)
    }
}

#[cfg(not(feature = "grammars"))]
mod imp {
    use std::path::Path;

    pub(super) const LANGUAGES: &[&str] = &[];

    pub(super) fn queries() -> Vec<&'static str> {
        Vec::new()
    }

    pub(super) fn language(_: &Path) -> Option<()> {
        None
    }

    pub(super) fn definitions(_: &Path, _: &[u8]) -> Option<Vec<super::Def>> {
        None
    }

    pub(super) fn outline(_: &Path, _: &[u8]) -> Option<Vec<super::Located>> {
        None
    }

    pub(super) fn syntax_errors(_: &Path, _: &[u8]) -> Option<usize> {
        None
    }
}

#[cfg(all(test, feature = "grammars"))]
mod tests {
    use super::*;

    #[test]
    fn lines_are_found_from_byte_offsets() {
        let source = b"one\ntwo\n\nfour";
        let starts = line_starts(source);
        assert_eq!(line_of(&starts, 0), 1);
        assert_eq!(line_of(&starts, 3), 1);
        assert_eq!(line_of(&starts, 4), 2);
        assert_eq!(line_of(&starts, 8), 3);
        assert_eq!(line_of(&starts, 9), 4);
        assert_eq!(line_text(source, &starts, 2), "two");
        assert_eq!(line_text(source, &starts, 9), "");
    }

    #[test]
    fn a_long_line_is_cut_on_a_character_boundary() {
        let source = "é".repeat(TEXT_MAX).into_bytes();
        let text = line_text(&source, &line_starts(&source), 1);
        assert!(text.ends_with("..."));
        assert!(text.len() <= TEXT_MAX + 3);
    }
}

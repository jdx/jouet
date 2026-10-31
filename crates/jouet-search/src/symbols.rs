//! The definitions in a source file, from the definition patterns of each
//! grammar's own tags query (`TAGS_QUERY`; TypeScript's together with
//! JavaScript's, as tree-sitter's CLI combines them). Search uses them to
//! group matches by the definition they are in.
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

/// The names of the languages this build supports.
pub fn languages() -> &'static [&'static str] {
    imp::LANGUAGES
}

/// A hash of every query this build runs, so definitions cached by an older
/// build are read again.
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
    use super::{Def, line_of, line_starts, line_text};
    use std::cell::RefCell;
    use std::collections::BTreeMap;
    use std::path::Path;
    use std::sync::OnceLock;
    use tree_sitter::{Parser, Query, QueryCursor, StreamingIterator};

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

    /// A language's grammar and tags query.
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

    pub(super) fn definitions(path: &Path, source: &[u8]) -> Option<Vec<Def>> {
        let lang = language(path)?;
        let query = definitions_query(lang)?;
        let tree = PARSER.with_borrow_mut(|parser| {
            parser.set_language(&source_language(lang)).ok()?;
            parser.parse(source, None)
        })?;
        let names = query.capture_names();
        let starts = line_starts(source);
        let mut cursor = QueryCursor::new();
        // One definition per node: the earliest pattern's, as
        // tree-sitter's tags tool picks it.
        let mut found: BTreeMap<(usize, usize), (usize, Def)> = BTreeMap::new();
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
                    Def {
                        name: text.to_string(),
                        kind: kind.to_string(),
                        line,
                        start: line_of(&starts, range.start),
                        end: line_of(&starts, end),
                        text: line_text(source, &starts, line),
                    },
                ),
            );
        }
        let mut defs: Vec<Def> = found.into_values().map(|(_, def)| def).collect();
        defs.sort_by_key(|d| (d.start, d.line));
        Some(defs)
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

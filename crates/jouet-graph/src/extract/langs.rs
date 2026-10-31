//! Grammar registry: a codegraph language name → its tree-sitter grammar.
//!
//! Ported from codegraph-kernel's `langs.rs`, trimmed to the languages jouet
//! extracts. `tsx` and `jsx` reuse another language's grammar exactly the way
//! codegraph's map does.

use tree_sitter::Language;

pub(crate) fn grammar_for(language: &str) -> Option<Language> {
    match language {
        "typescript" => Some(tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()),
        "tsx" => Some(tree_sitter_typescript::LANGUAGE_TSX.into()),
        "javascript" | "jsx" => Some(tree_sitter_javascript::LANGUAGE.into()),
        "java" => Some(tree_sitter_java::LANGUAGE.into()),
        "python" => Some(tree_sitter_python::LANGUAGE.into()),
        "go" => Some(tree_sitter_go::LANGUAGE.into()),
        "c" => Some(tree_sitter_c::LANGUAGE.into()),
        "cpp" => Some(tree_sitter_cpp::LANGUAGE.into()),
        "rust" => Some(tree_sitter_rust::LANGUAGE.into()),
        "ruby" => Some(tree_sitter_ruby::LANGUAGE.into()),
        _ => None,
    }
}

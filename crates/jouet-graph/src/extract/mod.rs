//! Extraction: one file's source to its symbols, the edges between them,
//! and the references that linking resolves later.
//!
//! The walkers are ported from codegraph-kernel (MIT), codegraph's Rust
//! tree-sitter extraction, with napi removed. Each language keeps its own
//! module and codegraph's structure and comments, so a later codegraph fix
//! can be compared and carried over.

/// First statement of every recursive walker function: once the stack is
/// spent, stop descending (see [`stack`]).
macro_rules! stack_guard {
    () => {
        if $crate::extract::stack::exhausted() {
            return ::core::default::Default::default();
        }
    };
}

mod buffers;
mod ccpp;
mod docstring;
mod go;
mod ids;
mod java;
mod langs;
mod python;
mod ruby;
mod rustlang;
mod stack;
mod textutil;
mod tsjs;

pub use buffers::{Edge, Extraction, Node, Ref};
pub(crate) use stack::STACK_SIZE;

use regex::Regex;
use std::sync::OnceLock;

/// The languages jouet extracts, by codegraph's names.
pub const LANGUAGES: [&str; 11] = [
    "typescript",
    "tsx",
    "javascript",
    "jsx",
    "python",
    "go",
    "rust",
    "java",
    "ruby",
    "c",
    "cpp",
];

/// The language of a path by its extension (codegraph's `EXTENSION_MAP`,
/// for the languages jouet extracts), or `None`.
pub fn language_for_path(path: &str) -> Option<&'static str> {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let ext = name.rsplit_once('.')?.1.to_ascii_lowercase();
    Some(match ext.as_str() {
        "ts" | "mts" | "cts" => "typescript",
        "tsx" => "tsx",
        "js" | "mjs" | "cjs" => "javascript",
        "jsx" => "jsx",
        "py" | "pyw" => "python",
        "go" => "go",
        "rs" => "rust",
        "java" => "java",
        "rb" | "rake" => "ruby",
        "c" | "h" => "c",
        "cpp" | "cc" | "cxx" | "hpp" | "hxx" | "cu" | "cuh" => "cpp",
        _ => return None,
    })
}

/// The language to extract `source` as: [`language_for_path`], refined by
/// content the way codegraph's `detectLanguage` does (a Flow-typed `.js`
/// parses as TSX; a `.h` that looks like C++ is C++).
pub fn detect_language(path: &str, source: &str) -> Option<&'static str> {
    let lang = language_for_path(path)?;
    if matches!(lang, "javascript" | "jsx") && has_flow_pragma(source) {
        return Some("tsx");
    }
    if lang == "c" && path.to_ascii_lowercase().ends_with(".h") && looks_like_cpp(source) {
        return Some("cpp");
    }
    Some(lang)
}

fn has_flow_pragma(source: &str) -> bool {
    static LEAD: OnceLock<Regex> = OnceLock::new();
    static FLOW: OnceLock<Regex> = OnceLock::new();
    static NOFLOW: OnceLock<Regex> = OnceLock::new();
    let mut head = &source[..floor_char_boundary(source, 4096)];
    if head.starts_with("#!") {
        head = head.split_once('\n').map_or("", |(_, rest)| rest);
    }
    let lead = LEAD
        .get_or_init(|| Regex::new(r"^(?:\s*(?://[^\n]*|/\*(?s:.)*?\*/))*").unwrap())
        .find(head)
        .map_or("", |m| m.as_str());
    FLOW.get_or_init(|| Regex::new(r"@flow\b").unwrap())
        .is_match(lead)
        && !NOFLOW
            .get_or_init(|| Regex::new(r"@noflow\b").unwrap())
            .is_match(lead)
}

fn looks_like_cpp(source: &str) -> bool {
    static SAMPLE: OnceLock<Regex> = OnceLock::new();
    static BASE: OnceLock<Regex> = OnceLock::new();
    static COMMENT: OnceLock<Regex> = OnceLock::new();
    let sample = &source[..floor_char_boundary(source, 8192)];
    let signal = SAMPLE.get_or_init(|| {
        Regex::new(
            r"\bnamespace\b|\bclass\s+\w+\s*[:{]|\b(?:class|struct)\s+[A-Z][A-Z0-9_]+\s+\w+\s*(?:final\s*)?[:{]|\btemplate\s*<|\b(?:public|private|protected)\s*:|\bvirtual\b|\busing\s+(?:namespace\b|\w+\s*=)",
        )
        .unwrap()
    });
    if signal.is_match(sample) {
        return true;
    }
    let code = COMMENT
        .get_or_init(|| Regex::new(r"/\*(?s:.)*?\*/|//[^\n]*").unwrap())
        .replace_all(source, " ");
    BASE.get_or_init(|| {
        Regex::new(
            r"\b(?:class|struct)\s+\w+\s*(?:final\s*)?:\s*(?:(?:public|protected|private|virtual)\s+)*[A-Za-z_][\w:]*(?:\s*<[^{};]*>)?\s*[{,]",
        )
        .unwrap()
    })
    .is_match(&code)
}

fn floor_char_boundary(s: &str, max: usize) -> usize {
    if s.len() <= max {
        return s.len();
    }
    (0..=max)
        .rev()
        .find(|&i| s.is_char_boundary(i))
        .unwrap_or(0)
}

/// Extracts one file. `path` is repo-relative with `/` separators: it is
/// part of every node ID. Returns `None` for a language jouet does not
/// extract, and an error for a file it could not walk.
///
/// Runs on a thread of its own, sized for the walkers' recursion.
pub fn extract(path: &str, source: &str) -> Option<Result<Extraction, String>> {
    let language = detect_language(path, source)?;
    let result = std::thread::scope(|scope| {
        std::thread::Builder::new()
            .stack_size(stack::STACK_SIZE)
            .spawn_scoped(scope, || extract_on_this_thread(path, source, language))
            .map_err(|e| format!("spawning an extraction thread: {e}"))?
            .join()
            .map_err(|_| "extraction panicked".to_string())?
    });
    Some(result)
}

/// [`extract`] without the thread: the caller must be on a thread spawned
/// with [`stack::STACK_SIZE`].
pub(crate) fn extract_on_this_thread(
    path: &str,
    source: &str,
    language: &str,
) -> Result<Extraction, String> {
    stack::run_guarded(|| match language {
        "java" => java::extract(path, source),
        "python" => python::extract(path, source),
        "go" => go::extract(path, source),
        "c" | "cpp" => ccpp::extract(path, source, language),
        "rust" => rustlang::extract(path, source),
        "ruby" => ruby::extract(path, source),
        _ => tsjs::extract(path, source, language),
    })
}

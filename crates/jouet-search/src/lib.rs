//! Budgeted search with session memory: the engine behind `jouet search`.
//!
//! Search embeds ripgrep's libraries (`ignore`, `grep-searcher`,
//! `grep-regex`). Matches are grouped by the definitions tree-sitter finds,
//! for the languages the `grammars` feature compiles in; without it every
//! file is plain text.

pub mod memory;
pub mod search;
pub mod structural;
pub mod symbols;

//! Budgeted search, symbols and a repository map: the engine behind `jouet
//! search` and `jouet read`.
//!
//! Search embeds ripgrep's libraries (`ignore`, `grep-searcher`,
//! `grep-regex`). Symbols come from tree-sitter's tags queries, for the
//! languages the `grammars` feature compiles in; without it every file is
//! plain text. The repository map follows Aider's design (see [`map`]).

pub mod map;
pub mod memory;
pub mod search;
pub mod structural;
pub mod symbols;

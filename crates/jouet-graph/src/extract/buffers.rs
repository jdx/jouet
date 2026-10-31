//! The rows a walker emits, and the typed [`Extraction`] they become.
//!
//! codegraph-kernel encoded these rows as fixed-width little-endian buffers
//! for one napi boundary crossing per file. jouet has no boundary to cross,
//! so the rows stay structs, but the walkers' API is kept exactly (`Arena`,
//! `StrRef`, `Tables::push_node` and friends), so a walker here can still be
//! compared line by line with its codegraph original.

/// "Field absent" for a row index, line or column.
pub(crate) const NONE: u32 = 0xFFFF_FFFF;

/// Mirror of `NODE_KINDS` in codegraph's `src/types.ts`.
pub(crate) const NODE_KINDS: [&str; 23] = [
    "file",
    "module",
    "class",
    "struct",
    "interface",
    "trait",
    "protocol",
    "function",
    "method",
    "property",
    "field",
    "variable",
    "constant",
    "enum",
    "enum_member",
    "type_alias",
    "namespace",
    "parameter",
    "import",
    "export",
    "route",
    "component",
    "union",
];

/// Mirror of `EDGE_KINDS` in codegraph's `src/types.ts`.
pub(crate) const EDGE_KINDS: [&str; 13] = [
    "contains",
    "calls",
    "imports",
    "exports",
    "extends",
    "implements",
    "references",
    "type_of",
    "returns",
    "instantiates",
    "overrides",
    "decorates",
    "navigates",
];

/// Reference kind code for codegraph's internal-only `function_ref` (#756):
/// a function named as a value (`map(handler)`), resolved like a call.
pub(crate) const FUNCTION_REF_CODE: u8 = 200;

pub(crate) fn node_kind_index(kind: &str) -> Option<u8> {
    NODE_KINDS.iter().position(|k| *k == kind).map(|i| i as u8)
}

pub(crate) fn edge_kind_index(kind: &str) -> Option<u8> {
    EDGE_KINDS.iter().position(|k| *k == kind).map(|i| i as u8)
}

/// (offset, len) into the [`Arena`]. [`NONE_STR`] is an absent field.
pub(crate) type StrRef = (u32, u32);
pub(crate) const NONE_STR: StrRef = (NONE, 0);

/// The strings of one file's rows.
#[derive(Default)]
pub(crate) struct Arena {
    buf: String,
}

impl Arena {
    pub(crate) fn put(&mut self, s: &str) -> StrRef {
        let off = self.buf.len() as u32;
        self.buf.push_str(s);
        (off, s.len() as u32)
    }

    /// A NUL-joined list; absent when the list is empty.
    pub(crate) fn put_list(&mut self, items: &[String]) -> StrRef {
        if items.is_empty() {
            return NONE_STR;
        }
        self.put(&items.join("\0"))
    }

    fn get(&self, r: StrRef) -> Option<&str> {
        if r.0 == NONE {
            return None;
        }
        let start = r.0 as usize;
        self.buf.get(start..start + r.1 as usize)
    }
}

/// Tri-state booleans packed as (present, value) bit pairs.
#[derive(Default, Clone, Copy)]
pub(crate) struct BoolFlags(pub(crate) u16);

impl BoolFlags {
    pub(crate) fn set(&mut self, pair: u16, value: bool) {
        self.0 |= 1 << (pair * 2);
        if value {
            self.0 |= 1 << (pair * 2 + 1);
        }
    }

    fn get(self, pair: u16) -> bool {
        self.0 & (1 << (pair * 2 + 1)) != 0
    }
}

pub(crate) const FLAG_IS_EXPORTED: u16 = 0;
pub(crate) const FLAG_IS_ASYNC: u16 = 1;
pub(crate) const FLAG_IS_STATIC: u16 = 2;
pub(crate) const FLAG_IS_ABSTRACT: u16 = 3;

#[derive(Clone, Copy)]
pub(crate) struct NodeRow {
    pub(crate) kind: u8,
    pub(crate) visibility: u8,
    pub(crate) flags: BoolFlags,
    pub(crate) start_line: u32,
    pub(crate) end_line: u32,
    pub(crate) start_column: u32,
    pub(crate) end_column: u32,
    pub(crate) name: StrRef,
    pub(crate) qualified_name: StrRef,
    pub(crate) id: StrRef,
    pub(crate) docstring: StrRef,
    pub(crate) signature: StrRef,
    pub(crate) decorators: StrRef,
    pub(crate) type_parameters: StrRef,
    pub(crate) return_type: StrRef,
    pub(crate) extra_json: StrRef,
}

#[derive(Clone, Copy)]
pub(crate) struct EdgeRow {
    pub(crate) source_idx: u32,
    pub(crate) target_idx: u32,
    pub(crate) kind: u8,
    pub(crate) provenance: u8,
    pub(crate) line: u32,
    pub(crate) column: u32,
    pub(crate) metadata_json: StrRef,
    pub(crate) source_id_str: StrRef,
    pub(crate) target_id_str: StrRef,
}

#[derive(Clone, Copy)]
pub(crate) struct RefRow {
    pub(crate) from_idx: u32,
    pub(crate) kind: u8,
    pub(crate) line: u32,
    pub(crate) column: u32,
    pub(crate) reference_name: StrRef,
    pub(crate) candidates: StrRef,
    pub(crate) from_id_str: StrRef,
}

#[derive(Default)]
pub(crate) struct Tables {
    nodes: Vec<NodeRow>,
    edges: Vec<EdgeRow>,
    refs: Vec<RefRow>,
}

impl Tables {
    pub(crate) fn push_node(&mut self, r: &NodeRow) -> u32 {
        self.nodes.push(*r);
        self.nodes.len() as u32 - 1
    }

    pub(crate) fn push_edge(&mut self, r: &EdgeRow) {
        self.edges.push(*r);
    }

    pub(crate) fn push_ref(&mut self, r: &RefRow) {
        self.refs.push(*r);
    }

    /// codegraph sets a flag here for refs that carry the extracting file's
    /// path; every row in an [`Extraction`] belongs to one file anyway.
    pub(crate) fn push_ref_flagged(&mut self, r: &RefRow, _flags: u8) {
        self.refs.push(*r);
    }
}

/// codegraph's ref-row flag for a ruby mixin `implements` ref.
pub(crate) const REF_FLAG_FILE_PATH: u8 = 1;

/// A symbol in one file: a function, class, field, import, or the file itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    pub kind: &'static str,
    pub name: String,
    pub qualified_name: String,
    /// `kind:hash`, or `file:PATH` for the file node: stable while the
    /// symbol keeps its file, kind, name and line.
    pub id: String,
    /// 1-based.
    pub start_line: u32,
    pub end_line: u32,
    /// 0-based, in UTF-16 code units (codegraph's convention).
    pub start_column: u32,
    pub end_column: u32,
    pub docstring: Option<String>,
    pub signature: Option<String>,
    pub visibility: Option<&'static str>,
    pub is_exported: bool,
    pub is_async: bool,
    pub is_static: bool,
    pub is_abstract: bool,
    pub decorators: Vec<String>,
    pub type_parameters: Vec<String>,
    pub return_type: Option<String>,
    /// codegraph's escape hatch: JSON of any further properties.
    pub extra: Option<String>,
}

/// A relationship found inside one file, between two of its nodes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edge {
    /// Indexes into [`Extraction::nodes`].
    pub source: usize,
    pub target: usize,
    pub kind: &'static str,
    pub line: Option<u32>,
    pub column: Option<u32>,
    pub metadata: Option<String>,
    /// `tree-sitter` or `heuristic`, when the walker said.
    pub provenance: Option<&'static str>,
}

/// A name used in one file that linking resolves to a definition, possibly
/// in another file: a call, an import, a base class.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ref {
    /// Index into [`Extraction::nodes`] of the enclosing symbol.
    pub from: usize,
    /// An edge kind, or `function_ref`.
    pub kind: &'static str,
    pub name: String,
    pub line: u32,
    pub column: u32,
    pub candidates: Vec<String>,
}

/// Everything extraction found in one file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Extraction {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub refs: Vec<Ref>,
    /// The parse tree had errors; tree-sitter recovered and the walk went on.
    pub has_errors: bool,
}

fn opt(arena: &Arena, r: StrRef) -> Option<String> {
    arena.get(r).map(str::to_string)
}

fn list(arena: &Arena, r: StrRef) -> Vec<String> {
    arena
        .get(r)
        .map(|s| s.split('\0').map(str::to_string).collect())
        .unwrap_or_default()
}

fn coord(v: u32) -> Option<u32> {
    (v != NONE).then_some(v)
}

const VISIBILITY: [&str; 4] = ["public", "private", "protected", "internal"];

/// Turns a walker's rows into an [`Extraction`]. Every edge and ref the
/// ported walkers emit names its nodes by row; one naming them by ID
/// string (codegraph's cross-file escape hatch) is dropped.
pub(crate) fn finish(tables: Tables, arena: Arena) -> Extraction {
    let count = tables.nodes.len();
    let nodes = tables
        .nodes
        .iter()
        .map(|r| Node {
            kind: NODE_KINDS
                .get(r.kind as usize)
                .copied()
                .unwrap_or("variable"),
            name: opt(&arena, r.name).unwrap_or_default(),
            qualified_name: opt(&arena, r.qualified_name).unwrap_or_default(),
            id: opt(&arena, r.id).unwrap_or_default(),
            start_line: r.start_line,
            end_line: r.end_line,
            start_column: r.start_column,
            end_column: r.end_column,
            docstring: opt(&arena, r.docstring),
            signature: opt(&arena, r.signature),
            visibility: (r.visibility as usize)
                .checked_sub(1)
                .and_then(|i| VISIBILITY.get(i).copied()),
            is_exported: r.flags.get(FLAG_IS_EXPORTED),
            is_async: r.flags.get(FLAG_IS_ASYNC),
            is_static: r.flags.get(FLAG_IS_STATIC),
            is_abstract: r.flags.get(FLAG_IS_ABSTRACT),
            decorators: list(&arena, r.decorators),
            type_parameters: list(&arena, r.type_parameters),
            return_type: opt(&arena, r.return_type),
            extra: opt(&arena, r.extra_json),
        })
        .collect();
    let edges = tables
        .edges
        .iter()
        .filter(|r| {
            r.source_id_str == NONE_STR
                && r.target_id_str == NONE_STR
                && (r.source_idx as usize) < count
                && (r.target_idx as usize) < count
        })
        .map(|r| Edge {
            source: r.source_idx as usize,
            target: r.target_idx as usize,
            kind: EDGE_KINDS
                .get(r.kind as usize)
                .copied()
                .unwrap_or("references"),
            line: coord(r.line),
            column: coord(r.column),
            metadata: opt(&arena, r.metadata_json),
            provenance: match r.provenance {
                1 => Some("tree-sitter"),
                3 => Some("heuristic"),
                _ => None,
            },
        })
        .collect();
    let refs = tables
        .refs
        .iter()
        .filter(|r| r.from_id_str == NONE_STR && (r.from_idx as usize) < count)
        .map(|r| Ref {
            from: r.from_idx as usize,
            kind: if r.kind == FUNCTION_REF_CODE {
                "function_ref"
            } else {
                EDGE_KINDS
                    .get(r.kind as usize)
                    .copied()
                    .unwrap_or("references")
            },
            name: opt(&arena, r.reference_name).unwrap_or_default(),
            line: r.line,
            column: r.column,
            candidates: list(&arena, r.candidates),
        })
        .collect();
    Extraction {
        nodes,
        edges,
        refs,
        has_errors: false,
    }
}

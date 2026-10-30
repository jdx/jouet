//! Parsing edit specs: search/replace blocks, Codex-style patches, and JSON.
//!
//! Every format parses into the same [`Spec`], so matching and writing never
//! care which one the agent wrote. A spec that ends inside an open block or
//! section is an error, never a shorter spec: a heredoc delimiter that appears
//! in the content cuts the spec short, and applying the part that survived
//! would be a partial edit.

use serde::Deserialize;
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Blocks,
    Patch,
    Json,
}

impl Format {
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "blocks" => Some(Self::Blocks),
            "patch" => Some(Self::Patch),
            "json" => Some(Self::Json),
            _ => None,
        }
    }

    /// Picks the format from the first non-blank line.
    pub fn detect(text: &str) -> Option<Self> {
        let first = text.lines().map(str::trim).find(|l| !l.is_empty())?;
        if first.starts_with("*** Begin Patch") {
            Some(Self::Patch)
        } else if first.starts_with('[') || first.starts_with('{') {
            Some(Self::Json)
        } else if first.starts_with("===") {
            Some(Self::Blocks)
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Count {
    Exactly(usize),
    /// Every match, of which there must be at least one.
    All,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Edit {
    /// Replace occurrences of `old`. Block edits match whole lines; JSON and
    /// command-line edits match anywhere, like `str::replace`.
    Replace {
        old: String,
        new: String,
        count: Count,
        line_aligned: bool,
    },
    /// A patch hunk. It is searched for after the previous hunk, and after
    /// each anchor line in turn, and must match exactly once in what remains.
    Hunk {
        anchors: Vec<String>,
        old: Vec<String>,
        new: Vec<String>,
        at_eof: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileOp {
    Update {
        path: String,
        edits: Vec<Edit>,
        move_to: Option<String>,
    },
    Add {
        path: String,
        content: String,
    },
    Delete {
        path: String,
    },
}

impl FileOp {
    pub fn path(&self) -> &str {
        match self {
            Self::Update { path, .. } | Self::Add { path, .. } | Self::Delete { path } => path,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Spec {
    pub ops: Vec<FileOp>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpecError {
    /// 1-based line in the spec, when the error has one.
    pub line: Option<usize>,
    pub message: String,
}

impl SpecError {
    fn at(line: usize, message: impl Into<String>) -> Self {
        Self {
            line: Some(line),
            message: message.into(),
        }
    }

    fn new(message: impl Into<String>) -> Self {
        Self {
            line: None,
            message: message.into(),
        }
    }
}

impl fmt::Display for SpecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.line {
            Some(line) => write!(f, "spec line {line}: {}", self.message),
            None => f.write_str(&self.message),
        }
    }
}

impl std::error::Error for SpecError {}

pub fn parse(text: &str, format: Format) -> Result<Spec, SpecError> {
    let spec = match format {
        Format::Blocks => parse_blocks(text)?,
        Format::Patch => parse_patch(text)?,
        Format::Json => parse_json(text)?,
    };
    if spec.ops.is_empty() {
        return Err(SpecError::new("the spec contains no edits"));
    }
    Ok(spec)
}

impl Spec {
    /// Appends an update edit, merging it into an earlier update of the same
    /// file so that edits to one file always apply in order.
    fn push_edit(&mut self, path: &str, edit: Edit) {
        for op in self.ops.iter_mut().rev() {
            if let FileOp::Update {
                path: p,
                edits,
                move_to: None,
            } = op
                && p == path
            {
                edits.push(edit);
                return;
            }
        }
        self.ops.push(FileOp::Update {
            path: path.to_string(),
            edits: vec![edit],
            move_to: None,
        });
    }
}

fn join_lines(lines: &[&str]) -> String {
    let mut s = String::new();
    for line in lines {
        s.push_str(line);
        s.push('\n');
    }
    s
}

fn parse_blocks(text: &str) -> Result<Spec, SpecError> {
    let lines: Vec<&str> = text.lines().collect();
    let mut spec = Spec::default();
    let mut file: Option<&str> = None;
    let mut i = 0;
    while i < lines.len() {
        let n = i + 1;
        let line = lines[i];
        if let Some(path) = line.strip_prefix("===") {
            let path = path.trim();
            if path.is_empty() || path.starts_with('=') {
                return Err(SpecError::at(n, "expected `=== PATH`"));
            }
            file = Some(path);
            i += 1;
            continue;
        }
        if line.trim().is_empty() {
            i += 1;
            continue;
        }
        let Some((width, count)) = parse_open_fence(line).map_err(|m| SpecError::at(n, m))? else {
            return Err(SpecError::at(
                n,
                "unexpected text outside a block; expected `=== PATH` or `<<<<<<< SEARCH`",
            ));
        };
        let Some(path) = file else {
            return Err(SpecError::at(n, "block before any `=== PATH` line"));
        };
        let separator = "=".repeat(width);
        let close = format!("{} REPLACE", ">".repeat(width));
        let search_start = i + 1;
        let sep = (search_start..lines.len())
            .find(|&j| lines[j] == separator)
            .ok_or_else(|| {
                SpecError::at(
                    n,
                    format!("the spec ends inside the block opened here (no `{separator}` line)"),
                )
            })?;
        let end = (sep + 1..lines.len())
            .find(|&j| lines[j] == close)
            .ok_or_else(|| {
                SpecError::at(
                    n,
                    format!("the spec ends inside the block opened here (no `{close}` line)"),
                )
            })?;
        let old = join_lines(&lines[search_start..sep]);
        let new = join_lines(&lines[sep + 1..end]);
        if old.is_empty() {
            if count.is_some() {
                return Err(SpecError::at(n, "an empty search cannot have a count"));
            }
            spec.ops.push(FileOp::Add {
                path: path.to_string(),
                content: new,
            });
        } else {
            spec.push_edit(
                path,
                Edit::Replace {
                    old,
                    new,
                    count: count.unwrap_or(Count::Exactly(1)),
                    line_aligned: true,
                },
            );
        }
        i = end + 1;
    }
    Ok(spec)
}

/// Recognizes `<<<<<<< SEARCH`, with a fence of seven or more `<`, and an
/// optional `count=N` or `count=all`. Returns the fence width and the count.
fn parse_open_fence(line: &str) -> Result<Option<(usize, Option<Count>)>, String> {
    let width = line.bytes().take_while(|&b| b == b'<').count();
    if width < 7 {
        return Ok(None);
    }
    let mut words = line[width..].split_whitespace();
    if words.next() != Some("SEARCH") {
        return Ok(None);
    }
    let count = match words.next() {
        None => None,
        Some(word) => {
            let value = word.strip_prefix("count=").ok_or_else(|| {
                format!("unknown option `{word}` (expected `count=N` or `count=all`)")
            })?;
            Some(match value {
                "all" => Count::All,
                n => match n.parse::<usize>() {
                    Ok(n) if n > 0 => Count::Exactly(n),
                    _ => return Err(format!("`count={n}` must be a positive number or `all`")),
                },
            })
        }
    };
    if let Some(extra) = words.next() {
        return Err(format!("unexpected `{extra}` after the count"));
    }
    Ok(Some((width, count)))
}

fn parse_patch(text: &str) -> Result<Spec, SpecError> {
    let lines: Vec<&str> = text.lines().collect();
    let start = lines
        .iter()
        .position(|l| l.trim() == "*** Begin Patch")
        .ok_or_else(|| SpecError::new("missing `*** Begin Patch`"))?;
    let end = lines
        .iter()
        .rposition(|l| l.trim() == "*** End Patch")
        .filter(|&e| e > start)
        .ok_or_else(|| {
            SpecError::new("the patch has no `*** End Patch` line, so it may have been cut short; nothing was applied")
        })?;
    if let Some(j) = (end + 1..lines.len()).find(|&j| !lines[j].trim().is_empty()) {
        return Err(SpecError::at(j + 1, "text after `*** End Patch`"));
    }
    let mut spec = Spec::default();
    let mut i = start + 1;
    while i < end {
        let n = i + 1;
        let line = lines[i];
        if line.trim().is_empty() {
            i += 1;
        } else if let Some(path) = line.strip_prefix("*** Add File: ") {
            let mut content = String::new();
            i += 1;
            while i < end && !lines[i].starts_with("*** ") {
                let body = lines[i].strip_prefix('+').ok_or_else(|| {
                    SpecError::at(i + 1, "lines of an added file must start with `+`")
                })?;
                content.push_str(body);
                content.push('\n');
                i += 1;
            }
            spec.ops.push(FileOp::Add {
                path: path.trim().to_string(),
                content,
            });
        } else if let Some(path) = line.strip_prefix("*** Delete File: ") {
            spec.ops.push(FileOp::Delete {
                path: path.trim().to_string(),
            });
            i += 1;
        } else if let Some(path) = line.strip_prefix("*** Update File: ") {
            i += 1;
            let mut move_to = None;
            if i < end
                && let Some(to) = lines[i].strip_prefix("*** Move to: ")
            {
                move_to = Some(to.trim().to_string());
                i += 1;
            }
            let (edits, next) = parse_hunks(&lines, i, end)?;
            if edits.is_empty() && move_to.is_none() {
                return Err(SpecError::at(n, "an update with no hunks"));
            }
            spec.ops.push(FileOp::Update {
                path: path.trim().to_string(),
                edits,
                move_to,
            });
            i = next;
        } else {
            return Err(SpecError::at(
                n,
                "expected `*** Add File:`, `*** Delete File:` or `*** Update File:`",
            ));
        }
    }
    Ok(spec)
}

#[derive(Default)]
struct HunkBuilder {
    anchors: Vec<String>,
    old: Vec<String>,
    new: Vec<String>,
    at_eof: bool,
    has_lines: bool,
}

impl HunkBuilder {
    fn finish(self, out: &mut Vec<Edit>) {
        if self.has_lines || !self.anchors.is_empty() {
            out.push(Edit::Hunk {
                anchors: self.anchors,
                old: self.old,
                new: self.new,
                at_eof: self.at_eof,
            });
        }
    }
}

fn parse_hunks(lines: &[&str], mut i: usize, end: usize) -> Result<(Vec<Edit>, usize), SpecError> {
    let mut edits = Vec::new();
    let mut hunk = HunkBuilder::default();
    while i < end {
        let line = lines[i];
        if line.starts_with("*** End of File") {
            hunk.at_eof = true;
        } else if line.starts_with("*** ") {
            break;
        } else if let Some(anchor) = line.strip_prefix("@@") {
            if hunk.has_lines {
                std::mem::take(&mut hunk).finish(&mut edits);
            }
            let anchor = anchor.trim();
            if !anchor.is_empty() {
                hunk.anchors.push(anchor.to_string());
            }
        } else if let Some(rest) = line.strip_prefix('+') {
            hunk.new.push(rest.to_string());
            hunk.has_lines = true;
        } else if let Some(rest) = line.strip_prefix('-') {
            hunk.old.push(rest.to_string());
            hunk.has_lines = true;
        } else if let Some(rest) = line.strip_prefix(' ') {
            hunk.old.push(rest.to_string());
            hunk.new.push(rest.to_string());
            hunk.has_lines = true;
        } else if line.is_empty() {
            // An empty context line whose leading space was trimmed.
            hunk.old.push(String::new());
            hunk.new.push(String::new());
            hunk.has_lines = true;
        } else {
            return Err(SpecError::at(
                i + 1,
                "hunk lines must start with ` `, `-`, `+` or `@@`",
            ));
        }
        i += 1;
    }
    hunk.finish(&mut edits);
    Ok((edits, i))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct JsonEdit {
    path: String,
    #[serde(default)]
    old: String,
    new: String,
    #[serde(default)]
    count: Option<JsonCount>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum JsonCount {
    N(usize),
    Word(String),
}

fn parse_json(text: &str) -> Result<Spec, SpecError> {
    let entries: Vec<JsonEdit> = serde_json::from_str(text)
        .map_err(|e| SpecError::at(e.line(), format!("invalid JSON spec: {e}")))?;
    let mut spec = Spec::default();
    for (index, e) in entries.into_iter().enumerate() {
        let count = match e.count {
            None => Count::Exactly(1),
            Some(JsonCount::N(n)) if n > 0 => Count::Exactly(n),
            Some(JsonCount::Word(w)) if w == "all" => Count::All,
            Some(_) => {
                return Err(SpecError::new(format!(
                    "entry {index}: `count` must be a positive number or \"all\""
                )));
            }
        };
        if e.old.is_empty() {
            spec.ops.push(FileOp::Add {
                path: e.path,
                content: e.new,
            });
        } else {
            spec.push_edit(
                &e.path,
                Edit::Replace {
                    old: e.old,
                    new: e.new,
                    count,
                    line_aligned: false,
                },
            );
        }
    }
    Ok(spec)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn replace(old: &str, new: &str, count: Count, line_aligned: bool) -> Edit {
        Edit::Replace {
            old: old.into(),
            new: new.into(),
            count,
            line_aligned,
        }
    }

    #[test]
    fn detects_each_format_from_its_first_line() {
        assert_eq!(Format::detect("\n*** Begin Patch\n"), Some(Format::Patch));
        assert_eq!(Format::detect("  [{\"path\":\"a\"}]"), Some(Format::Json));
        assert_eq!(Format::detect("=== src/a.rs\n"), Some(Format::Blocks));
        assert_eq!(Format::detect("hello"), None);
    }

    #[test]
    fn blocks_merge_edits_to_one_file_in_order() {
        let spec = parse(
            "=== a.rs\n<<<<<<< SEARCH\none\n=======\n1\n>>>>>>> REPLACE\n\n=== b.rs\n<<<<<<< SEARCH count=all\nx\n=======\ny\n>>>>>>> REPLACE\n=== a.rs\n<<<<<<< SEARCH count=2\ntwo\n=======\n>>>>>>> REPLACE\n",
            Format::Blocks,
        )
        .unwrap();
        assert_eq!(
            spec.ops,
            vec![
                FileOp::Update {
                    path: "a.rs".into(),
                    edits: vec![
                        replace("one\n", "1\n", Count::Exactly(1), true),
                        replace("two\n", "", Count::Exactly(2), true)
                    ],
                    move_to: None
                },
                FileOp::Update {
                    path: "b.rs".into(),
                    edits: vec![replace("x\n", "y\n", Count::All, true)],
                    move_to: None
                },
            ]
        );
    }

    #[test]
    fn a_longer_fence_lets_content_contain_conflict_markers() {
        let spec = parse(
            "=== a\n<<<<<<<<<< SEARCH\n<<<<<<< HEAD\n=======\n==========\nclean\n>>>>>>>>>> REPLACE\n",
            Format::Blocks,
        )
        .unwrap();
        assert_eq!(
            spec.ops[0],
            FileOp::Update {
                path: "a".into(),
                edits: vec![replace(
                    "<<<<<<< HEAD\n=======\n",
                    "clean\n",
                    Count::Exactly(1),
                    true
                )],
                move_to: None
            }
        );
    }

    #[test]
    fn an_empty_search_creates_the_file() {
        let spec = parse(
            "=== new.txt\n<<<<<<< SEARCH\n=======\nhi\n>>>>>>> REPLACE\n",
            Format::Blocks,
        )
        .unwrap();
        assert_eq!(
            spec.ops,
            vec![FileOp::Add {
                path: "new.txt".into(),
                content: "hi\n".into()
            }]
        );
    }

    #[test]
    fn a_spec_cut_short_inside_a_block_is_refused() {
        let err = parse("=== a\n<<<<<<< SEARCH\nold\n=======\nnew\n", Format::Blocks).unwrap_err();
        assert_eq!(err.line, Some(2));
        assert!(err.message.contains("ends inside the block"), "{err}");
    }

    #[test]
    fn prose_outside_blocks_is_refused() {
        let err = parse("=== a\nHere is the change:\n", Format::Blocks).unwrap_err();
        assert_eq!(err.line, Some(2));
    }

    #[test]
    fn bad_counts_are_refused() {
        for bad in ["count=0", "count=some", "times=2"] {
            let text = format!("=== a\n<<<<<<< SEARCH {bad}\nx\n=======\ny\n>>>>>>> REPLACE\n");
            assert!(parse(&text, Format::Blocks).is_err(), "{bad}");
        }
    }

    #[test]
    fn patch_parses_every_operation() {
        let spec = parse(
            "*** Begin Patch\n*** Add File: new.txt\n+hello\n*** Delete File: old.txt\n*** Update File: src/a.rs\n*** Move to: src/b.rs\n@@ fn main() {\n     let x = 1;\n-    let y = 2;\n+    let y = 3;\n@@\n context\n-gone\n*** End of File\n*** End Patch\n",
            Format::Patch,
        )
        .unwrap();
        assert_eq!(
            spec.ops,
            vec![
                FileOp::Add {
                    path: "new.txt".into(),
                    content: "hello\n".into()
                },
                FileOp::Delete {
                    path: "old.txt".into()
                },
                FileOp::Update {
                    path: "src/a.rs".into(),
                    edits: vec![
                        Edit::Hunk {
                            anchors: vec!["fn main() {".into()],
                            old: vec!["    let x = 1;".into(), "    let y = 2;".into()],
                            new: vec!["    let x = 1;".into(), "    let y = 3;".into()],
                            at_eof: false
                        },
                        Edit::Hunk {
                            anchors: vec![],
                            old: vec!["context".into(), "gone".into()],
                            new: vec!["context".into()],
                            at_eof: true
                        },
                    ],
                    move_to: Some("src/b.rs".into())
                },
            ]
        );
    }

    #[test]
    fn a_patch_without_its_end_line_is_refused() {
        let err = parse("*** Begin Patch\n*** Delete File: a\n", Format::Patch).unwrap_err();
        assert!(err.message.contains("End Patch"), "{err}");
    }

    #[test]
    fn json_edits_match_substrings() {
        let spec = parse(
            r#"[{"path":"a","old":"x","new":"y","count":"all"},{"path":"b","new":"created"}]"#,
            Format::Json,
        )
        .unwrap();
        assert_eq!(
            spec.ops,
            vec![
                FileOp::Update {
                    path: "a".into(),
                    edits: vec![replace("x", "y", Count::All, false)],
                    move_to: None
                },
                FileOp::Add {
                    path: "b".into(),
                    content: "created".into()
                },
            ]
        );
    }

    #[test]
    fn an_empty_spec_is_refused() {
        assert!(parse("[]", Format::Json).is_err());
        assert!(parse("\n\n", Format::Blocks).is_err());
    }
}

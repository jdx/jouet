//! `jouet read FILE`: a file by its outline, a symbol or a range; and
//! `jouet read --map`, Aider's repository map in Rust.

use super::Output;
use crate::ReadArgs;
use jouet_output::exit::Exit;
use jouet_search::map::{self, DEFAULT_BUDGET, UNFOCUSED_MULTIPLIER};
use jouet_search::memory::{self, Memory, Stamp};
use jouet_search::symbols::{self, Located, SelectError, Selector};
use serde::Serialize;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// A file this long or shorter is printed whole when no mode is given.
const WHOLE: usize = 300;
/// The default for --max-lines.
const MAX_LINES: usize = 400;

pub(crate) fn run(args: &ReadArgs, out: &Output) -> Exit {
    if !args.map {
        return read_file(args, out);
    }
    if args.outline || args.symbol.is_some() || args.range.is_some() {
        return out.error(
            Exit::Usage,
            "--map reads a directory; --outline, --symbol and --range read a file",
        );
    }
    if symbols::languages().is_empty() {
        return out.error_with_hint(
            Exit::Usage,
            "this jouet was built without grammars, so it cannot read definitions",
            Some("install a release build, or build with `--features grammars`"),
        );
    }
    let root = PathBuf::from(args.path.as_deref().unwrap_or("."));
    if !root.is_dir() {
        return out.error(
            Exit::NotFound,
            &format!("{}: no such directory", root.display()),
        );
    }
    let (focus_files, focus_idents) = split_focus(&root, &args.focus);
    let budget = args
        .budget
        .unwrap_or(if focus_files.is_empty() && focus_idents.is_empty() {
            DEFAULT_BUDGET * UNFOCUSED_MULTIPLIER
        } else {
            DEFAULT_BUDGET
        });
    let options = map::Options {
        root,
        focus_files,
        focus_idents,
        budget,
        cache: jouet_output::paths::cache_dir().map(|d| d.join("tags")),
    };
    let repo_map = map::build(&options);
    out.result(&repo_map, || map::render(&repo_map));
    Exit::Ok
}

/// Splits `--focus` values into files, relative to the map's root, and
/// identifiers: a value naming an existing file is a file.
fn split_focus(root: &Path, focus: &[String]) -> (Vec<String>, Vec<String>) {
    let canonical_root = std::fs::canonicalize(root).ok();
    let mut files = Vec::new();
    let mut idents = Vec::new();
    for value in focus {
        let path = Path::new(value);
        let file = [path.to_path_buf(), root.join(path)]
            .into_iter()
            .find(|p| p.is_file())
            .and_then(|p| std::fs::canonicalize(p).ok());
        let rel = file.zip(canonical_root.as_ref()).and_then(|(file, root)| {
            file.strip_prefix(root).ok().map(|rel| {
                rel.components()
                    .map(|c| c.as_os_str().to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("/")
            })
        });
        match rel {
            Some(rel) => files.push(rel),
            None => idents.push(value.clone()),
        }
    }
    (files, idents)
}

#[derive(Serialize)]
struct FileRead {
    path: String,
    /// The file's length in lines.
    lines: usize,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    outline: Vec<OutlineEntry>,
    /// Definitions left out of the outline by --max-lines.
    #[serde(skip_serializing_if = "is_zero")]
    more_definitions: usize,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    sections: Vec<Section>,
}

#[derive(Serialize)]
struct OutlineEntry {
    kind: String,
    name: String,
    start: u32,
    end: u32,
}

#[derive(Serialize)]
struct Section {
    /// The definition this section is, for --symbol.
    #[serde(skip_serializing_if = "Option::is_none")]
    symbol: Option<String>,
    start: usize,
    end: usize,
    /// The lines, without numbers; empty when they were already shown.
    #[serde(skip_serializing_if = "String::is_empty")]
    text: String,
    /// Lines left out by --max-lines.
    #[serde(skip_serializing_if = "is_zero")]
    more: usize,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    already_shown: bool,
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

fn read_file(args: &ReadArgs, out: &Output) -> Exit {
    let Some(path) = args.path.as_deref() else {
        return out.error(
            Exit::Usage,
            "give a FILE to read, with --outline, --symbol NAME or --range A:B, or --map",
        );
    };
    let modes = usize::from(args.outline)
        + usize::from(args.symbol.is_some())
        + usize::from(args.range.is_some());
    if modes > 1 {
        return out.error(Exit::Usage, "give one of --outline, --symbol and --range");
    }
    if args.budget.is_some() || !args.focus.is_empty() {
        return out.error(Exit::Usage, "--budget and --focus are for --map");
    }
    let file = Path::new(path);
    let bytes = match std::fs::read(file) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return out.error(Exit::NotFound, &format!("{path}: no such file"));
        }
        Err(e) => return out.error(Exit::Failed, &format!("{path}: {e}")),
    };
    let text = String::from_utf8_lossy(&bytes);
    let lines: Vec<&str> = text.lines().collect();
    let mut read = FileRead {
        path: path.to_string(),
        lines: lines.len(),
        outline: Vec::new(),
        more_definitions: 0,
        sections: Vec::new(),
    };
    let max = args.max_lines.unwrap_or(MAX_LINES);
    let mut memory = if args.again {
        Memory::none()
    } else {
        super::search::session_memory()
    };
    let stamp = Stamp::read(file);
    let key = memory::key(file);
    let section = |symbol: Option<String>, start: usize, end: usize, memory: &mut Memory| {
        let end = end.min(lines.len());
        let seen =
            stamp.is_some_and(|stamp| memory.shown_range(&key, stamp, to_u32(start), to_u32(end)));
        if seen {
            return Section {
                symbol,
                start,
                end,
                text: String::new(),
                more: 0,
                already_shown: true,
            };
        }
        let shown_end = if max == 0 {
            end
        } else {
            end.min(start + max - 1)
        };
        if let Some(stamp) = stamp {
            memory.record(&key, stamp, to_u32(start), to_u32(shown_end));
        }
        Section {
            symbol,
            start,
            end: shown_end,
            text: lines[start - 1..shown_end].join("\n"),
            more: end - shown_end,
            already_shown: false,
        }
    };
    let outline = || symbols::outline(file, &bytes);
    if let Some(range) = &args.range {
        let Some((start, end)) = parse_range(range, lines.len()) else {
            return out.error(
                Exit::Usage,
                &format!(
                    "--range takes A:B, A: or :B within the file's {} lines, not `{range}`",
                    lines.len()
                ),
            );
        };
        read.sections.push(section(None, start, end, &mut memory));
    } else if let Some(name) = &args.symbol {
        let selector = match Selector::parse(name) {
            Ok(selector) => selector,
            Err(e) => return out.error(Exit::Usage, &select_error(&e, path)),
        };
        match symbols::select(file, &bytes, &selector) {
            Ok(found) => {
                for l in found {
                    let label = describe(&l);
                    read.sections.push(section(
                        Some(label),
                        l.def.start as usize,
                        l.def.end as usize,
                        &mut memory,
                    ));
                }
            }
            Err(SelectError::NotFound) => {
                let defs: Vec<String> =
                    outline().unwrap_or_default().iter().map(describe).collect();
                return out.error_with_hint(
                    Exit::NotFound,
                    &format!("{path} has no definition matching `{name}`"),
                    Some(&format!("it defines: {}", defs.join(", "))),
                );
            }
            Err(e) => return out.error(Exit::Usage, &select_error(&e, path)),
        }
    } else if args.outline || lines.len() > WHOLE {
        match outline() {
            Some(all) => read.outline = all.iter().map(entry).collect(),
            None if args.outline => {
                return out.error(Exit::Usage, &select_error(&SelectError::Unsupported, path));
            }
            // A long file jouet cannot outline: its start, with a marker.
            None => read
                .sections
                .push(section(None, 1, lines.len(), &mut memory)),
        }
    } else if !lines.is_empty() {
        read.sections
            .push(section(None, 1, lines.len(), &mut memory));
    }
    memory.save();
    let implicit_outline = !args.outline && args.symbol.is_none() && args.range.is_none();
    // An outline is held to --max-lines too.
    let outline_total = read.outline.len();
    if max != 0 && read.outline.len() > max {
        read.outline.truncate(max);
    }
    read.more_definitions = outline_total - read.outline.len();
    out.result(&read, || render(&read, implicit_outline));
    Exit::Ok
}

fn to_u32(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

fn describe(l: &Located) -> String {
    format!(
        "{} {} ({}-{})",
        l.def.kind,
        l.qualified(),
        l.def.start,
        l.def.end
    )
}

fn entry(l: &Located) -> OutlineEntry {
    OutlineEntry {
        kind: l.def.kind.clone(),
        name: l.qualified(),
        start: l.def.start,
        end: l.def.end,
    }
}

fn select_error(e: &SelectError, path: &str) -> String {
    match e {
        SelectError::Invalid(message) => message.clone(),
        SelectError::Unsupported => format!(
            "jouet cannot read definitions from {path}; use --range A:B (it reads {})",
            symbols::languages().join(", ")
        ),
        SelectError::NotFound => format!("{path} has no such definition"),
    }
}

/// `A:B`, `A:`, `:B` or `A`, 1-based and inclusive, within `len` lines.
fn parse_range(text: &str, len: usize) -> Option<(usize, usize)> {
    let (a, b) = text.split_once(':').unwrap_or((text, text));
    let start = if a.is_empty() {
        1
    } else {
        a.trim().parse().ok()?
    };
    let end = if b.is_empty() {
        len
    } else {
        b.trim().parse().ok()?
    };
    (start >= 1 && start <= end && start <= len).then_some((start, end.min(len)))
}

fn render(read: &FileRead, implicit_outline: bool) -> String {
    let mut text = String::new();
    if !read.outline.is_empty() || implicit_outline && read.sections.is_empty() {
        let _ = writeln!(
            text,
            "{}: {} lines, {} definitions",
            read.path,
            read.lines,
            read.outline.len() + read.more_definitions
        );
        let width = read
            .outline
            .iter()
            .map(|e| e.end.to_string().len() * 2 + 1)
            .max()
            .unwrap_or(0);
        for e in &read.outline {
            let range = format!("{}-{}", e.start, e.end);
            let _ = writeln!(text, "  {range:<width$}  {} {}", e.kind, e.name);
        }
        if read.more_definitions > 0 {
            let _ = writeln!(
                text,
                "... {} more definitions (raise --max-lines)",
                read.more_definitions
            );
        }
        if implicit_outline && read.lines > WHOLE {
            let _ = writeln!(
                text,
                "... the file has {} lines; this is its outline (--symbol NAME or --range A:B prints code)",
                read.lines
            );
        }
        return text;
    }
    for s in &read.sections {
        let what = match &s.symbol {
            Some(symbol) => format!("{}: {symbol}", read.path),
            None => format!("{}:{}-{}", read.path, s.start, s.end),
        };
        if s.already_shown {
            let _ = writeln!(text, "{what} already shown, unchanged (--again prints it)");
            continue;
        }
        if s.symbol.is_some() {
            let _ = writeln!(text, "{what}");
        }
        let width = (s.end + s.more).to_string().len();
        for (i, line) in s.text.lines().enumerate() {
            let _ = writeln!(text, "{:>width$}\t{line}", s.start + i);
        }
        if s.more > 0 {
            let _ = writeln!(
                text,
                "... {} more lines (--range {}:{}, or raise --max-lines)",
                s.more,
                s.end + 1,
                s.end + s.more
            );
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::parse_range;

    #[test]
    fn ranges_are_read_and_clamped() {
        assert_eq!(parse_range("3:5", 10), Some((3, 5)));
        assert_eq!(parse_range("3:", 10), Some((3, 10)));
        assert_eq!(parse_range(":4", 10), Some((1, 4)));
        assert_eq!(parse_range("7", 10), Some((7, 7)));
        assert_eq!(parse_range("8:99", 10), Some((8, 10)));
        assert_eq!(parse_range("11:12", 10), None);
        assert_eq!(parse_range("5:3", 10), None);
        assert_eq!(parse_range("0:3", 10), None);
        assert_eq!(parse_range("a:b", 10), None);
    }
}

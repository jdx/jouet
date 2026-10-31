//! `jouet docs PKG [SYMBOL]`: a dependency's API at the locked version.

use super::Output;
use crate::DocsArgs;
use crate::docs::{self, Index, Item, Lang, Missing, Package, summary};
use jouet_output::exit::Exit;
use serde::Serialize;
use std::fmt::Write as _;
use std::path::Path;

/// The default outline budget, in tokens of 4 bytes.
const BUDGET: usize = 2048;
/// Matches shown in full; the rest are listed.
const SHOWN: usize = 3;
/// Matches listed after those shown.
const LISTED: usize = 20;
/// Lines of a definition's body shown.
const EXCERPT: usize = 12;
/// Lines of a doc comment shown.
const DOC_LINES: usize = 40;
/// A type's members shown.
const MEMBERS: usize = 40;

#[derive(Serialize)]
struct Report<'a> {
    package: &'a str,
    version: &'a str,
    language: Lang,
    lockfile: String,
    source: String,
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    notes: &'a [String],
    #[serde(skip_serializing_if = "Option::is_none")]
    summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    symbol: Option<&'a str>,
    /// The outline (of the package, or of a module SYMBOL names).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    items: Vec<Entry>,
    #[serde(skip_serializing_if = "is_zero")]
    more_items: usize,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    matches: Vec<Match>,
    /// Matches beyond those shown, by path.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    more_matches: Vec<Brief>,
    /// Matches beyond those listed.
    #[serde(skip_serializing_if = "is_zero")]
    unlisted_matches: usize,
}

#[derive(Serialize)]
struct Entry {
    kind: String,
    path: String,
    signature: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    summary: Option<String>,
    /// Relative to the package's source.
    file: String,
    line: u32,
    #[serde(skip_serializing_if = "is_zero")]
    members: usize,
}

#[derive(Serialize)]
struct Match {
    kind: String,
    path: String,
    /// Other paths the package exposes it by.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    also: Vec<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    private: bool,
    file: String,
    line: u32,
    end_line: u32,
    signature: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    doc: Option<String>,
    /// The start of its body, after the signature (and a docstring).
    #[serde(skip_serializing_if = "String::is_empty")]
    excerpt: String,
    #[serde(skip_serializing_if = "is_zero")]
    excerpt_line: usize,
    #[serde(skip_serializing_if = "is_zero")]
    more_lines: usize,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    members: Vec<Member>,
    /// For a re-export of another package's definition: the lookup that
    /// reads it there.
    #[serde(skip_serializing_if = "Option::is_none")]
    see: Option<String>,
}

#[derive(Serialize)]
struct Member {
    kind: String,
    name: String,
    signature: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    summary: Option<String>,
    line: u32,
}

#[derive(Serialize)]
struct Brief {
    kind: String,
    path: String,
    file: String,
    line: u32,
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

pub(crate) fn run(args: &DocsArgs, out: &Output) -> Exit {
    let lang = match args.lang.as_deref().map(|l| (l, Lang::parse(l))) {
        None => None,
        Some((_, Some(lang))) => Some(lang),
        Some((text, None)) => {
            return out.error(
                Exit::Usage,
                &format!("--lang takes rust, ts, python or go, not `{text}`"),
            );
        }
    };
    if args.package.is_empty() {
        return out.error(Exit::Usage, "give the package to look up");
    }
    let cwd = std::env::current_dir().unwrap_or_default();
    let pkg = match docs::locate(&cwd, &args.package, lang) {
        Ok(pkg) => pkg,
        Err(Missing::NotFound { message, hint }) => {
            return out.error_with_hint(Exit::NotFound, &message, hint.as_deref());
        }
        Err(Missing::Precondition { message, hint }) => {
            return out.error_with_hint(Exit::Precondition, &message, hint.as_deref());
        }
    };
    let index = match docs::index(&pkg) {
        Ok(index) => index,
        Err(e) => return out.error(Exit::Failed, &e),
    };
    let mut report = Report {
        package: &pkg.name,
        version: &pkg.version,
        language: pkg.lang,
        lockfile: shown_path(&cwd, &pkg.lockfile),
        source: pkg.api_root.display().to_string(),
        notes: &pkg.notes,
        summary: None,
        symbol: args.symbol.as_deref(),
        items: Vec::new(),
        more_items: 0,
        matches: Vec::new(),
        more_matches: Vec::new(),
        unlisted_matches: 0,
    };
    let budget = args.budget.unwrap_or(BUDGET).saturating_mul(4);
    let Some(symbol) = args.symbol.as_deref() else {
        report.summary = index.doc.as_deref().map(summary);
        (report.items, report.more_items) = outline(&index, args.private, None, budget);
        out.result(&report, || render(&report, &pkg));
        return Exit::Ok;
    };
    let found = docs::find(&index, symbol, args.private, rooted(&pkg));
    if found.is_empty() {
        return not_found(&index, &pkg, symbol, args.private, out);
    }
    // A module: its outline.
    if let [one] = found.as_slice()
        && index.items[*one].kind == "module"
    {
        let item = &index.items[*one];
        report.summary = item.doc.as_deref().map(summary);
        (report.items, report.more_items) = outline(
            &index,
            args.private,
            item.paths.first().map(String::as_str),
            budget,
        );
        report.matches.push(full(&index, *one, &pkg, args.private));
        out.result(&report, || render(&report, &pkg));
        return Exit::Ok;
    }
    for &i in found.iter().take(SHOWN) {
        report.matches.push(full(&index, i, &pkg, args.private));
    }
    for &i in found.iter().skip(SHOWN).take(LISTED) {
        let item = &index.items[i];
        report.more_matches.push(Brief {
            kind: item.kind.clone(),
            path: item.paths.first().cloned().unwrap_or_default(),
            file: item.file.clone(),
            line: item.line,
        });
    }
    report.unlisted_matches = found.len().saturating_sub(SHOWN + LISTED);
    out.result(&report, || render(&report, &pkg));
    Exit::Ok
}

fn not_found(index: &Index, pkg: &Package, symbol: &str, private: bool, out: &Output) -> Exit {
    let what = format!("{} {}", pkg.name, pkg.version);
    if !private && !docs::find(index, symbol, true, rooted(pkg)).is_empty() {
        return out.error_with_hint(
            Exit::NotFound,
            &format!("{what} has no public definition matching `{symbol}`"),
            Some("it has a private one: --private shows it"),
        );
    }
    let near = docs::near(index, symbol, private, 10);
    let hint = if near.is_empty() {
        format!("`jouet docs {}` lists its API", pkg.name)
    } else {
        format!("similar: {}", near.join(", "))
    };
    out.error_with_hint(
        Exit::NotFound,
        &format!("{what} has no definition matching `{symbol}`"),
        Some(&hint),
    )
}

/// Whether the package's paths start with its own name (`ureq::Agent`,
/// `click.Context`, `kit.Job`); a `.d.ts` export is a bare name.
fn rooted(pkg: &Package) -> bool {
    pkg.lang != Lang::Ts
}

/// `path` relative to `cwd` when it is under it.
fn shown_path(cwd: &Path, path: &Path) -> String {
    path.strip_prefix(cwd).unwrap_or(path).display().to_string()
}

/// A signature on one line, for an outline.
fn one_line(signature: &str) -> String {
    let joined = signature
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace("( ", "(")
        .replace(", )", ")")
        .replace(" )", ")");
    let mut out: String = joined.chars().take(200).collect();
    if out.chars().count() < joined.chars().count() {
        out.push_str("...");
    }
    out
}

/// The outline's entries within `budget` bytes, and how many were left out.
fn outline(
    index: &Index,
    private: bool,
    within: Option<&str>,
    budget: usize,
) -> (Vec<Entry>, usize) {
    let all = docs::outline(index, private, within);
    let mut used = 0;
    let mut entries = Vec::new();
    for &i in &all {
        let item = &index.items[i];
        let entry = Entry {
            kind: item.kind.clone(),
            path: item.paths.first().cloned().unwrap_or_default(),
            signature: one_line(&item.signature),
            summary: item.doc.as_deref().map(summary).filter(|s| !s.is_empty()),
            file: item.file.clone(),
            line: item.line,
            members: docs::members(index, i, private).len(),
        };
        let size = entry_text(&entry).len();
        if used + size > budget && !entries.is_empty() {
            break;
        }
        used += size;
        entries.push(entry);
    }
    let more = all.len() - entries.len();
    (entries, more)
}

fn full(index: &Index, i: usize, pkg: &Package, private: bool) -> Match {
    let item = &index.items[i];
    let file = pkg.api_root.join(&item.file);
    let members = docs::members(index, i, private);
    // A declaration file's class is its members, listed below.
    let listed = pkg.lang == Lang::Ts && !members.is_empty();
    let (excerpt, excerpt_line, more_lines) =
        if item.kind == "module" || item.kind == "reexport" || listed {
            (String::new(), 0, 0)
        } else {
            body(&file, item, pkg.lang)
        };
    Match {
        see: (item.kind == "reexport")
            .then(|| elsewhere(&item.signature))
            .flatten(),
        kind: item.kind.clone(),
        path: item.paths.first().cloned().unwrap_or_default(),
        also: item.paths.iter().skip(1).cloned().collect(),
        private: !item.public,
        file: file.display().to_string(),
        line: item.line,
        end_line: item.end_line,
        signature: item.signature.clone(),
        doc: item.doc.as_deref().map(cap_doc),
        excerpt,
        excerpt_line,
        more_lines,
        members: members
            .iter()
            .map(|&m| {
                let member = &index.items[m];
                Member {
                    kind: member.kind.clone(),
                    name: member.name.clone(),
                    signature: one_line(&member.signature),
                    summary: member.doc.as_deref().map(summary).filter(|s| !s.is_empty()),
                    line: member.line,
                }
            })
            .collect(),
    }
}

/// `jouet docs serde_core Serialize` for `pub use serde_core::Serialize;`,
/// and `jouet docs typing Any` for `from typing import Any`.
fn elsewhere(declaration: &str) -> Option<String> {
    let (package, symbol) = if let Some(rest) = declaration.strip_prefix("from ") {
        let (module, name) = rest.split_once(" import ")?;
        (
            module.split('.').next()?.to_string(),
            name.trim().to_string(),
        )
    } else {
        let path = declaration.strip_prefix("pub use ")?.trim_end_matches(';');
        let path = path.split(" as ").next()?.trim().trim_start_matches("::");
        let (package, symbol) = path.split_once("::")?;
        (package.to_string(), symbol.to_string())
    };
    Some(format!("jouet docs {package} {symbol}"))
}

fn cap_doc(doc: &str) -> String {
    let lines: Vec<&str> = doc.lines().collect();
    if lines.len() <= DOC_LINES {
        return doc.to_string();
    }
    format!(
        "{}\n... {} more lines of documentation",
        lines[..DOC_LINES].join("\n"),
        lines.len() - DOC_LINES
    )
}

/// The definition's body after its signature (and, in Python, after its
/// docstring), at most [`EXCERPT`] lines: the text, its first line, and how
/// many lines were left out.
fn body(file: &Path, item: &Item, lang: Lang) -> (String, usize, usize) {
    let Ok(text) = std::fs::read_to_string(file) else {
        return (String::new(), 0, 0);
    };
    let lines: Vec<&str> = text.lines().collect();
    let end = (item.end_line as usize).min(lines.len());
    let mut start = item.line as usize + item.signature.lines().count().max(1);
    if lang == Lang::Python {
        start = after_docstring(&lines, start);
    }
    if start > end {
        return (String::new(), 0, 0);
    }
    let shown_end = end.min(start + EXCERPT - 1);
    let width = end.to_string().len();
    let mut out = String::new();
    for (n, line) in lines[start - 1..shown_end].iter().enumerate() {
        let _ = writeln!(out, "{:>width$}\t{line}", start + n);
    }
    (out, start, end - shown_end)
}

/// The first line (1-based) after a docstring that opens at `start`.
fn after_docstring(lines: &[&str], start: usize) -> usize {
    let Some(first) = lines.get(start.saturating_sub(1)) else {
        return start;
    };
    let opened = first.trim_start().trim_start_matches(['r', 'u', 'R', 'U']);
    let Some(quote) = ["\"\"\"", "'''"]
        .into_iter()
        .find(|q| opened.starts_with(q))
    else {
        return start;
    };
    if opened[3..].contains(quote) {
        return start + 1;
    }
    (start..lines.len())
        .find(|&i| lines[i].contains(quote))
        .map_or(start, |i| i + 2)
}

fn entry_text(e: &Entry) -> String {
    let mut text = format!("{} {} ({}:{}", e.kind, e.path, e.file, e.line);
    match e.members {
        0 => {}
        1 => text.push_str(", 1 member"),
        n => {
            let _ = write!(text, ", {n} members");
        }
    }
    text.push(')');
    if let Some(summary) = &e.summary {
        let _ = write!(text, ": {summary}");
    }
    text.push('\n');
    if !e.signature.is_empty() && e.kind != "module" {
        let _ = writeln!(text, "  {}", e.signature);
    }
    text
}

/// A path under the home directory as `~/...`, for text output.
fn tilde(path: &str) -> String {
    std::env::home_dir()
        .and_then(|home| {
            Some(format!(
                "~/{}",
                Path::new(path).strip_prefix(home).ok()?.display()
            ))
        })
        .unwrap_or_else(|| path.to_string())
}

fn render(r: &Report, pkg: &Package) -> String {
    let mut text = format!(
        "{} {} ({}, {}) at {}\n",
        r.package,
        r.version,
        pkg.lang.key(),
        r.lockfile,
        tilde(&r.source)
    );
    for note in r.notes {
        let _ = writeln!(text, "note: {note}");
    }
    if let Some(summary) = &r.summary {
        let _ = writeln!(text, "{summary}");
    }
    if !r.items.is_empty() || r.symbol.is_none() {
        text.push('\n');
        for e in &r.items {
            text.push_str(&entry_text(e));
        }
        if r.items.is_empty() {
            text.push_str("no public definitions (--private lists the rest)\n");
        }
        if r.more_items > 0 {
            let _ = writeln!(
                text,
                "... {} more definitions (raise --budget, or `jouet docs {} MODULE` for one module)",
                r.more_items, r.package
            );
        }
        return text;
    }
    for m in &r.matches {
        text.push('\n');
        let _ = writeln!(
            text,
            "{} {}{}  {}:{}-{}",
            m.kind,
            m.path,
            if m.private { " (private)" } else { "" },
            tilde(&m.file),
            m.line,
            m.end_line
        );
        if !m.also.is_empty() {
            let _ = writeln!(text, "also: {}", m.also.join(", "));
        }
        let _ = writeln!(text, "{}", m.signature);
        if let Some(see) = &m.see {
            let _ = writeln!(text, "defined in another package: `{see}` reads it there");
        }
        if let Some(doc) = &m.doc {
            text.push('\n');
            for line in doc.lines() {
                let _ = writeln!(text, "  {line}");
            }
        }
        if !m.excerpt.is_empty() {
            text.push('\n');
            text.push_str(&m.excerpt);
            if m.more_lines > 0 {
                let end = m.excerpt_line + m.excerpt.lines().count();
                let _ = writeln!(
                    text,
                    "... {} more lines (jouet read {} --range {}:{})",
                    m.more_lines,
                    tilde(&m.file),
                    end,
                    m.end_line
                );
            }
        }
        if !m.members.is_empty() {
            let _ = writeln!(text, "\nmembers:");
            for member in m.members.iter().take(MEMBERS) {
                let _ = write!(text, "  {}", member.signature);
                if let Some(summary) = &member.summary {
                    let _ = write!(text, "  // {summary}");
                }
                text.push('\n');
            }
            if m.members.len() > MEMBERS {
                let _ = writeln!(
                    text,
                    "  ... {} more members (--json lists them all)",
                    m.members.len() - MEMBERS
                );
            }
        }
    }
    if !r.more_matches.is_empty() {
        let _ = writeln!(
            text,
            "\n{} more matches:",
            r.more_matches.len() + r.unlisted_matches
        );
        for b in &r.more_matches {
            let _ = writeln!(text, "  {} {} ({}:{})", b.kind, b.path, b.file, b.line);
        }
        if r.unlisted_matches > 0 {
            let _ = writeln!(
                text,
                "  ... {} more (give more of the path)",
                r.unlisted_matches
            );
        }
    }
    text
}

//! What the code graph's extractor does not keep, read from the source
//! lines around a definition: the doc comment and attributes above it, and
//! its header (the declaration without its body).

/// The comment syntax a language documents a definition with.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Style {
    /// `///` and `/** */`, with `#[...]` attributes between them and the item.
    Rust,
    /// The nearest `/** */` above, with decorators between.
    Js,
    /// The `//` lines right above, without directives.
    Go,
}

/// Where a header ends.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Cut {
    /// At the first `{` or `;` outside brackets: a body follows.
    Body,
    /// At a `:` that ends a line outside brackets (Python).
    Colon,
    /// The whole declaration when it is short (a `.d.ts` member), else at
    /// its body.
    Short,
}

/// The longest header kept, in lines.
const HEADER_LINES: usize = 12;

/// The doc comment written above line `start` (1-based), and the attributes
/// between it and the definition, in source order.
pub(super) fn doc_above(
    lines: &[&str],
    start: usize,
    style: Style,
) -> (Option<String>, Vec<String>) {
    let mut doc: Vec<String> = Vec::new();
    let mut attrs: Vec<String> = Vec::new();
    // `i` is the 0-based index of the line below the one looked at.
    let mut i = start.saturating_sub(1).min(lines.len());
    while i > 0 {
        let line = lines[i - 1].trim();
        match style {
            Style::Rust => {
                if let Some(rest) = line.strip_prefix("///") {
                    if !rest.starts_with('/') {
                        doc.push(strip_space(rest).to_string());
                    }
                    i -= 1;
                } else if line.starts_with("#[") {
                    attrs.push(line.to_string());
                    i -= 1;
                } else if line.ends_with(']')
                    && let Some(open) = attribute_start(lines, i - 1)
                {
                    attrs.push(
                        lines[open..i]
                            .iter()
                            .map(|l| l.trim())
                            .collect::<Vec<_>>()
                            .join(" "),
                    );
                    i = open;
                } else if line.ends_with("*/") {
                    let Some(open) = (0..i)
                        .rev()
                        .find(|&j| lines[j].trim_start().starts_with("/*"))
                    else {
                        break;
                    };
                    if lines[open].trim_start().starts_with("/**") {
                        doc.extend(block_comment(&lines[open..i]).into_iter().rev());
                    }
                    i = open;
                } else if line.starts_with("//") {
                    // A plain comment between the doc and the item.
                    i -= 1;
                } else {
                    break;
                }
            }
            Style::Js => {
                if line.starts_with('@') {
                    attrs.push(line.to_string());
                    i -= 1;
                } else if line.ends_with("*/") {
                    let Some(open) = (0..i)
                        .rev()
                        .find(|&j| lines[j].trim_start().starts_with("/*"))
                    else {
                        break;
                    };
                    if lines[open].trim_start().starts_with("/**") {
                        doc.extend(block_comment(&lines[open..i]).into_iter().rev());
                    }
                    break;
                } else {
                    break;
                }
            }
            Style::Go => {
                if let Some(rest) = line.strip_prefix("//") {
                    if !rest.starts_with("go:") && !rest.starts_with("nolint") {
                        doc.push(strip_space(rest).to_string());
                    }
                    i -= 1;
                } else {
                    break;
                }
            }
        }
    }
    doc.reverse();
    attrs.reverse();
    (join_doc(&doc), attrs)
}

/// The first line of a multi-line attribute that ends on line `end`
/// (0-based), when one starts within a few lines above it.
fn attribute_start(lines: &[&str], end: usize) -> Option<usize> {
    (end.saturating_sub(12)..end)
        .rev()
        .take_while(|&j| !lines[j].trim_start().starts_with("///"))
        .find(|&j| lines[j].trim_start().starts_with("#["))
}

/// The text of a `/** ... */` comment, without its markers.
fn block_comment(lines: &[&str]) -> Vec<String> {
    lines
        .iter()
        .map(|l| {
            let l = l.trim();
            let l = l.strip_prefix("/**").unwrap_or(l);
            let l = l.strip_suffix("*/").unwrap_or(l).trim_end();
            let l = l.strip_prefix('*').unwrap_or(l);
            strip_space(l).to_string()
        })
        .collect()
}

fn strip_space(s: &str) -> &str {
    s.strip_prefix(' ').unwrap_or(s)
}

/// Doc lines as one text, without the blank lines around it.
pub(super) fn join_doc(lines: &[String]) -> Option<String> {
    let start = lines.iter().position(|l| !l.trim().is_empty())?;
    let end = lines.iter().rposition(|l| !l.trim().is_empty())?;
    Some(lines[start..=end].join("\n"))
}

/// A module's own doc: Rust's `//!` lines at the top of a file.
pub(super) fn inner_doc(text: &str) -> Option<String> {
    // The first run of `//!` lines: inner docs come before any item, but
    // after (or among) inner attributes that may span lines.
    let lines: Vec<String> = text
        .lines()
        .map(str::trim)
        .skip_while(|l| !l.starts_with("//!"))
        .take_while(|l| l.starts_with("//!") || l.starts_with("#!["))
        .filter_map(|l| l.strip_prefix("//!"))
        .map(|l| strip_space(l).to_string())
        .collect();
    join_doc(&lines)
}

/// A Python module's docstring: a string literal that opens the file.
pub(super) fn python_module_doc(text: &str) -> Option<String> {
    let body = text
        .lines()
        .skip_while(|l| {
            let l = l.trim();
            l.is_empty() || l.starts_with('#')
        })
        .collect::<Vec<_>>()
        .join("\n");
    let body = body.trim_start_matches(['r', 'u', 'R', 'U']);
    let quote = ["\"\"\"", "'''"]
        .into_iter()
        .find(|q| body.starts_with(q))?;
    let rest = &body[3..];
    let end = rest.find(quote)?;
    let lines: Vec<String> = dedent(&rest[..end]);
    join_doc(&lines)
}

/// Lines with the indentation they share (after the first) removed.
fn dedent(text: &str) -> Vec<String> {
    let lines: Vec<&str> = text.lines().collect();
    let indent = lines
        .iter()
        .skip(1)
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start().len())
        .min()
        .unwrap_or(0);
    lines
        .iter()
        .enumerate()
        .map(|(i, l)| {
            if i == 0 {
                l.trim().to_string()
            } else {
                l.get(indent..)
                    .unwrap_or(l.trim_start())
                    .trim_end()
                    .to_string()
            }
        })
        .collect()
}

/// The declaration on lines `start..=end` (1-based) without its body,
/// dedented.
pub(super) fn header(lines: &[&str], start: usize, end: usize, cut: Cut) -> String {
    let start = start.max(1);
    let end = end.min(lines.len());
    if start > end {
        return String::new();
    }
    let span = &lines[start - 1..end];
    if cut == Cut::Short && span.len() <= 6 {
        return tidy(span.join("\n").trim_end().trim_end_matches(';'));
    }
    let text = span.join("\n");
    let mut depth = 0i32;
    let mut quote: Option<char> = None;
    let mut chars = text.char_indices().peekable();
    let mut cut_at = text.len();
    while let Some((at, c)) = chars.next() {
        if let Some(q) = quote {
            if c == '\\' {
                chars.next();
            } else if c == q {
                quote = None;
            }
            continue;
        }
        match c {
            '"' | '`' => quote = Some(c),
            '\'' if cut == Cut::Colon => quote = Some(c),
            '(' | '[' => depth += 1,
            ')' | ']' => depth -= 1,
            '{' | ';' if depth <= 0 && cut != Cut::Colon => {
                cut_at = at;
                break;
            }
            ':' if depth <= 0 && cut == Cut::Colon => {
                let rest = text[at + 1..].lines().next().unwrap_or("").trim();
                if rest.is_empty() || rest.starts_with('#') {
                    cut_at = at;
                    break;
                }
            }
            _ => {}
        }
    }
    tidy(&text[..cut_at])
}

/// Dedented, trailing space removed, and at most [`HEADER_LINES`] lines.
fn tidy(text: &str) -> String {
    let lines = dedent_keep_first(text.trim_end());
    let mut out: Vec<String> = lines.into_iter().take(HEADER_LINES + 1).collect();
    if out.len() > HEADER_LINES {
        out.truncate(HEADER_LINES);
        out.push("...".to_string());
    }
    out.join("\n")
}

/// Like [`dedent`], but measuring the first line's indentation too, so a
/// method's header lines up with its own first line.
fn dedent_keep_first(text: &str) -> Vec<String> {
    let lines: Vec<&str> = text.lines().collect();
    let indent = lines
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start().len())
        .min()
        .unwrap_or(0);
    lines
        .iter()
        .map(|l| {
            l.get(indent..)
                .unwrap_or(l.trim_start())
                .trim_end()
                .to_string()
        })
        .collect()
}

/// A README paragraph that is not prose: HTML, badges, a lone heading.
fn preamble(paragraph: &str) -> bool {
    let heading = paragraph.starts_with('#') && !paragraph.contains('\n');
    heading
        || paragraph.starts_with('<')
        || paragraph.starts_with("[![")
        || paragraph.starts_with("![")
}

/// A doc's first sentence or line, for an outline.
pub(crate) fn summary(doc: &str) -> String {
    // Past a README's badges, HTML and title.
    let first = doc
        .split("\n\n")
        .map(str::trim)
        .find(|p| !p.is_empty() && !preamble(p))
        .unwrap_or("")
        .lines()
        .map(str::trim)
        .collect::<Vec<_>>()
        .join(" ");
    let sentence = first
        .find(". ")
        .map_or(first.as_str(), |at| &first[..=at])
        .trim();
    let mut out: String = sentence.chars().take(160).collect();
    if out.chars().count() < sentence.chars().count() {
        out.push_str("...");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rust_doc_is_read_above_its_attributes() {
        let src = "/// Agents keep state.\n///\n/// More.\n#[derive(Clone)]\n#[cfg_attr(\n    test,\n    derive(Debug)\n)]\npub struct Agent {\n    x: u8,\n}\n";
        let lines: Vec<&str> = src.lines().collect();
        let (doc, attrs) = doc_above(&lines, 9, Style::Rust);
        assert_eq!(doc.as_deref(), Some("Agents keep state.\n\nMore."));
        assert_eq!(attrs.len(), 2);
        assert_eq!(header(&lines, 9, 11, Cut::Body), "pub struct Agent");
    }

    #[test]
    fn a_jsdoc_is_the_nearest_block_comment() {
        let src = "/** Not this. */\nx;\n/**\n * Parses it.\n * @param a the text\n */\nexport function parse(a: string): { ok: boolean };\n";
        let lines: Vec<&str> = src.lines().collect();
        let (doc, _) = doc_above(&lines, 7, Style::Js);
        assert_eq!(doc.as_deref(), Some("Parses it.\n@param a the text"));
        assert_eq!(
            header(&lines, 7, 7, Cut::Short),
            "export function parse(a: string): { ok: boolean }"
        );
    }

    #[test]
    fn headers_stop_at_the_body() {
        let src = "    pub fn get<T>(&self, uri: T) -> Builder\n    where\n        T: Into<Uri>,\n    {\n        todo!()\n    }";
        let lines: Vec<&str> = src.lines().collect();
        assert_eq!(
            header(&lines, 1, 6, Cut::Body),
            "pub fn get<T>(&self, uri: T) -> Builder\nwhere\n    T: Into<Uri>,"
        );
        let py =
            "def get(self, url: str = \"a:b\", **kw: dict[str, int]) -> Response:\n    return 1";
        let lines: Vec<&str> = py.lines().collect();
        assert_eq!(
            header(&lines, 1, 2, Cut::Colon),
            "def get(self, url: str = \"a:b\", **kw: dict[str, int]) -> Response"
        );
    }

    #[test]
    fn summaries_are_the_first_sentence() {
        assert_eq!(
            summary("Makes a GET. Then more.\n\nDetails."),
            "Makes a GET."
        );
        assert_eq!(summary("One line\nwrapped"), "One line wrapped");
        assert_eq!(
            summary("<div>\n<a>x</a>\n</div>\n\n# ureq\n\nA simple, safe HTTP client.\n\nMore."),
            "A simple, safe HTTP client."
        );
        assert_eq!(
            python_module_doc("# c\n\"\"\"\nClick is simple.\n\nMore.\n\"\"\"\n").as_deref(),
            Some("Click is simple.\n\nMore.")
        );
        assert_eq!(
            inner_doc("#![allow(x)]\n//! A crate.\n//!\n//! More.\nuse x;").as_deref(),
            Some("A crate.\n\nMore.")
        );
        assert_eq!(
            inner_doc("#![allow(\n    a,\n    b\n)]\n\n//! Tokio.\nmod x;\n//! not this")
                .as_deref(),
            Some("Tokio.")
        );
    }
}

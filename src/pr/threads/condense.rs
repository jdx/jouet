//! Condensing comment bodies for listing.
//!
//! Review bots wrap a few sentences of finding in HTML comments holding
//! their state, badge images and `<details>` blocks of prompts and
//! walkthroughs. Condensing removes the comments and badges, collapses each
//! `<details>` to its summary and caps what is left at a number of lines,
//! with a marker saying how much was cut. Suggestion blocks are code to
//! apply, so they are always kept whole, even inside a collapsed `<details>`
//! or past the cap. Other code blocks are kept intact by the HTML rules (an
//! HTML comment inside a code block is code) but count toward the cap.

/// Stands in for a fenced code block while the text around it is rewritten.
const MARK: char = '\u{1}';

#[derive(Debug, PartialEq, Eq)]
pub struct Condensed {
    pub body: String,
    /// Whether anything other than comments and badges was left out: lines
    /// past the cap or the contents of a `<details>`.
    pub truncated: bool,
}

struct Fence {
    lines: Vec<String>,
    suggestion: bool,
    closed: bool,
}

pub fn condense(body: &str, max_lines: usize) -> Condensed {
    let body = body.replace("\r\n", "\n");
    let (text, fences) = take_fences(&body);
    let text = remove_html_comments(&text);
    let (text, collapsed) = collapse_details(&text, &fences);
    let text = remove_badges(&text);
    let mut lines = Vec::new();
    for line in text.lines() {
        let line = line.trim_end();
        if line.is_empty() && lines.last().is_none_or(|l: &&str| l.is_empty()) {
            continue;
        }
        lines.push(line);
    }
    while lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    let (body, cut) = cap(&lines, &fences, max_lines);
    Condensed {
        body,
        truncated: collapsed || cut,
    }
}

/// The contents of the first suggestion block.
pub fn suggestion(body: &str) -> Option<String> {
    let body = body.replace("\r\n", "\n");
    let (_, fences) = take_fences(&body);
    let fence = fences.into_iter().find(|f| f.suggestion)?;
    let end = fence.lines.len() - usize::from(fence.closed);
    Some(fence.lines[1..end].join("\n"))
}

/// Replaces each fenced code block with a line holding its index.
fn take_fences(body: &str) -> (String, Vec<Fence>) {
    let mut text = String::new();
    let mut fences: Vec<Fence> = Vec::new();
    let mut open: Option<(char, usize)> = None;
    for line in body.lines() {
        let trimmed = line.trim_start();
        if let Some((c, len)) = open {
            let fence = fences.last_mut().expect("an open fence was pushed");
            fence.lines.push(line.to_string());
            let run = trimmed.chars().take_while(|&x| x == c).count();
            if run >= len && trimmed[run..].trim().is_empty() {
                fence.closed = true;
                open = None;
            }
            continue;
        }
        let c = trimmed.chars().next().unwrap_or(' ');
        let run = trimmed.chars().take_while(|&x| x == c).count();
        if matches!(c, '`' | '~') && run >= 3 {
            let info = trimmed[run..].trim();
            open = Some((c, run));
            text.push(MARK);
            text.push_str(&fences.len().to_string());
            text.push(MARK);
            text.push('\n');
            fences.push(Fence {
                lines: vec![line.to_string()],
                suggestion: info.split_whitespace().next() == Some("suggestion"),
                closed: false,
            });
            continue;
        }
        text.push_str(line);
        text.push('\n');
    }
    (text, fences)
}

fn remove_html_comments(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("<!--") {
        let Some(len) = rest[start..].find("-->") else {
            break;
        };
        out.push_str(&rest[..start]);
        rest = &rest[start + len + 3..];
    }
    out.push_str(rest);
    out
}

/// Collapses every outermost `<details>` to `[details: SUMMARY]`, followed by
/// any suggestion blocks it held. Returns whether any was collapsed.
fn collapse_details(text: &str, fences: &[Fence]) -> (String, bool) {
    let mut out = String::new();
    let mut rest = text;
    let mut collapsed = false;
    while let Some(start) = find_tag(rest, "details") {
        let Some(end) = closing(&rest[start..], "details") else {
            break;
        };
        let block = &rest[start..start + end];
        out.push_str(&rest[..start]);
        let summary = find_tag(block, "summary")
            .and_then(|s| {
                let open = s + block[s..].find('>')? + 1;
                let close = open + find_close(&block[open..], "summary")?;
                Some(plain(&block[open..close]))
            })
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "details".into());
        out.push_str(&format!("\n[details: {summary}]\n"));
        for index in marks(block) {
            if fences.get(index).is_some_and(|f| f.suggestion) {
                out.push_str(&format!("{MARK}{index}{MARK}\n"));
            }
        }
        collapsed = true;
        rest = &rest[start + end..];
    }
    out.push_str(rest);
    (out, collapsed)
}

/// Removes badge images (SVGs and images from badge services) and
/// `<picture>` elements, then the links they leave empty. An `<img>` badge
/// with alt text, such as a reviewer's `P1`, becomes `[P1]`.
fn remove_badges(text: &str) -> String {
    let text = replace(text, &['<', '!'], |s| {
        if find_tag(s, "picture") == Some(0) {
            return closing(s, "picture").map(|len| (len, String::new()));
        }
        img_tag(s).or_else(|| markdown_image(s))
    });
    replace(&text, &['<', '['], |s| {
        empty_anchor(s).or_else(|| empty_markdown_link(s))
    })
}

/// Rewrites `text`, trying `at` wherever one of `starts` appears: it gives
/// the length to replace and what to put in its place. Spaces after a
/// removal are dropped where they would follow a line start or markup
/// (`** Minor**` is not bold).
fn replace(text: &str, starts: &[char], at: impl Fn(&str) -> Option<(usize, String)>) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(i) = rest.find(starts) {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        match at(rest) {
            Some((len, with)) => {
                out.push_str(&with);
                rest = &rest[len..];
                if with.is_empty() && out.chars().last().is_none_or(|c| "\n*_>([".contains(c)) {
                    rest = rest.trim_start_matches([' ', '\t']);
                }
            }
            None => {
                out.push_str(&rest[..1]);
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// A badge `<img ...>` tag at the start of `s`, and its alt text.
fn img_tag(s: &str) -> Option<(usize, String)> {
    if !s.get(..4)?.eq_ignore_ascii_case("<img") {
        return None;
    }
    let len = s.find('>')? + 1;
    let tag = &s[..len];
    if !is_badge(attr(tag, "src")?) {
        return None;
    }
    let alt = attr(tag, "alt").map(str::trim).unwrap_or_default();
    Some((
        len,
        if alt.is_empty() {
            String::new()
        } else {
            format!("[{alt}]")
        },
    ))
}

/// A badge `![alt](url)` at the start of `s`.
fn markdown_image(s: &str) -> Option<(usize, String)> {
    let s2 = s.strip_prefix("![")?;
    let alt = s2.find("](")?;
    if s2[..alt].contains('\n') {
        return None;
    }
    let url_start = alt + 2;
    let url_len = s2[url_start..].find(')')?;
    let url = s2[url_start..url_start + url_len]
        .split_whitespace()
        .next()?;
    is_badge(url).then(|| (2 + url_start + url_len + 1, String::new()))
}

fn is_badge(url: &str) -> bool {
    let url = url.to_ascii_lowercase();
    let path = url.split(['?', '#']).next().unwrap_or_default();
    path.ends_with(".svg")
        || ["shields.io", "badgen.net", "/badge"]
            .iter()
            .any(|b| url.contains(b))
}

/// An `<a ...>` at the start of `s` that holds only whitespace (removed) or
/// links nowhere, `href="#"` (replaced by what it holds).
fn empty_anchor(s: &str) -> Option<(usize, String)> {
    let b = s.as_bytes();
    let anchor = b.len() > 2
        && b[0] == b'<'
        && b[1].eq_ignore_ascii_case(&b'a')
        && (b[2] == b'>' || b[2].is_ascii_whitespace());
    if !anchor {
        return None;
    }
    let open = s.find('>')? + 1;
    let close = open + find_close(&s[open..], "a")?;
    let inner = s[open..close].trim();
    let len = close + "</a>".len();
    if inner.is_empty() {
        Some((len, String::new()))
    } else {
        (attr(&s[..open], "href") == Some("#")).then(|| (len, inner.to_string()))
    }
}

/// A `[](url)` at the start of `s`.
fn empty_markdown_link(s: &str) -> Option<(usize, String)> {
    let r = s.strip_prefix('[')?.trim_start().strip_prefix("](")?;
    let len = r.find(')')?;
    (!r[..len].contains(char::is_whitespace)).then(|| (s.len() - r.len() + len + 1, String::new()))
}

/// Fits the lines into `max` lines (0 means no limit), expanding fences.
/// Suggestion blocks are always kept and do not count. A code block cut
/// short is closed so the rest of the text does not read as code.
fn cap(lines: &[&str], fences: &[Fence], max: usize) -> (String, bool) {
    let room = |kept: usize| {
        if max == 0 {
            usize::MAX
        } else {
            max.saturating_sub(kept)
        }
    };
    let mut out: Vec<String> = Vec::new();
    let (mut kept, mut cut) = (0, 0);
    let mut marker_at = None;
    for line in lines {
        match mark(line).and_then(|i| fences.get(i)) {
            Some(fence) if fence.suggestion => out.extend(fence.lines.iter().cloned()),
            Some(fence) => {
                let take = fence.lines.len().min(room(kept));
                out.extend(fence.lines[..take].iter().cloned());
                kept += take;
                if take < fence.lines.len() {
                    if take > 0 {
                        let open = fence.lines[0].trim_start();
                        let c = open.chars().next().unwrap_or('`');
                        let len = open.chars().take_while(|&x| x == c).count();
                        out.push(c.to_string().repeat(len));
                    }
                    marker_at.get_or_insert(out.len());
                    cut += fence.lines.len() - take;
                }
            }
            None if room(kept) > 0 => {
                out.push((*line).to_string());
                kept += 1;
            }
            None => {
                marker_at.get_or_insert(out.len());
                cut += 1;
            }
        }
    }
    if let Some(at) = marker_at {
        let s = if cut == 1 { "" } else { "s" };
        out.insert(at, format!("... {cut} more line{s} (--full shows all)"));
    }
    (out.join("\n"), cut > 0)
}

fn mark(line: &str) -> Option<usize> {
    line.strip_prefix(MARK)?.strip_suffix(MARK)?.parse().ok()
}

fn marks(text: &str) -> Vec<usize> {
    text.lines().filter_map(mark).collect()
}

/// The byte offset of the first `<NAME` tag (followed by `>` or whitespace),
/// ignoring case.
fn find_tag(s: &str, name: &str) -> Option<usize> {
    let lower = s.to_ascii_lowercase();
    let needle = format!("<{name}");
    let mut from = 0;
    while let Some(i) = lower[from..].find(&needle) {
        let at = from + i;
        let next = lower[at + needle.len()..].chars().next();
        if next.is_some_and(|c| c == '>' || c.is_whitespace()) {
            return Some(at);
        }
        from = at + needle.len();
    }
    None
}

/// The byte offset of the first `</NAME>` in `s`, ignoring case.
fn find_close(s: &str, name: &str) -> Option<usize> {
    s.to_ascii_lowercase().find(&format!("</{name}>"))
}

/// The length of the element that starts `s`, through its matching close
/// tag, counting nested elements of the same name.
fn closing(s: &str, name: &str) -> Option<usize> {
    let close = format!("</{name}>");
    let mut depth = 0;
    let mut at = 0;
    loop {
        let next_open = find_tag(&s[at..], name).map(|i| at + i);
        let next_close = find_close(&s[at..], name).map(|i| at + i)?;
        match next_open {
            Some(open) if open < next_close => {
                depth += 1;
                at = open + 1;
            }
            _ => {
                depth -= 1;
                at = next_close + close.len();
                if depth == 0 {
                    return Some(at);
                }
            }
        }
    }
}

/// An attribute's value in an HTML tag, quoted or not.
fn attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let lower = tag.to_ascii_lowercase();
    let mut from = 0;
    loop {
        let i = from + lower[from..].find(name)?;
        from = i + name.len();
        let before = lower[..i].chars().last();
        if !before.is_some_and(char::is_whitespace) {
            continue;
        }
        let rest = tag[from..].trim_start().strip_prefix('=')?.trim_start();
        return Some(match rest.chars().next()? {
            q @ ('"' | '\'') => {
                let rest = &rest[1..];
                &rest[..rest.find(q)?]
            }
            _ => rest.split(|c: char| c.is_whitespace() || c == '>').next()?,
        });
    }
}

/// Text with tags removed and whitespace collapsed.
fn plain(html: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn condensed(body: &str) -> String {
        condense(body, 30).body
    }

    #[test]
    fn html_comments_go_but_code_keeps_them() {
        let body = "Fix this.<!-- state: {\"a\":1}\nmore state -->\n\n```html\n<!-- kept -->\n```";
        assert_eq!(condensed(body), "Fix this.\n\n```html\n<!-- kept -->\n```");
    }

    #[test]
    fn details_collapse_to_their_summary() {
        let body = "Problem.\n\n<details>\n<summary>🤖 Prompt for <b>AI</b> agents</summary>\n\n\
                    Long prompt\n<details><summary>inner</summary>x</details>\n</details>\n\nAfter.";
        let c = condense(body, 30);
        assert_eq!(
            c.body,
            "Problem.\n\n[details: 🤖 Prompt for AI agents]\n\nAfter."
        );
        assert!(c.truncated);
    }

    #[test]
    fn suggestions_survive_a_collapsed_details() {
        let body = "Use a timeout.\n\n<details>\n<summary>📝 Committable suggestion</summary>\n\n\
                    ```suggestion\nlet resp = get(url).timeout(t)?;\n```\n\n</details>";
        assert_eq!(
            condensed(body),
            "Use a timeout.\n\n[details: 📝 Committable suggestion]\n\
             ```suggestion\nlet resp = get(url).timeout(t)?;\n```"
        );
        assert_eq!(
            suggestion(body).as_deref(),
            Some("let resp = get(url).timeout(t)?;")
        );
    }

    #[test]
    fn badges_and_their_empty_links_go() {
        let body = "<a href=\"https://cursor.com/open?x=1\"><picture><source srcset=\"a.svg\">\
                    <img alt=\"Fix in Cursor\" src=\"https://cursor.com/fix-in-cursor.svg\"></picture></a> \
                    [![CI](https://img.shields.io/badge/ci-passing-green)](https://ci) \
                    <img src=\"https://example.com/badge.svg?x=1\" />\n\
                    A real finding.\n\n![screenshot](https://example.com/shot.png)";
        assert_eq!(
            condensed(body),
            "A real finding.\n\n![screenshot](https://example.com/shot.png)"
        );
    }

    #[test]
    fn badge_alt_text_is_kept_and_markup_stays_valid() {
        // As greptile and CodeRabbit write them.
        let body = "<a href=\"#\"><img alt=\"P1\" src=\"https://x.s3.amazonaws.com/badges/p1.svg?v=9\" \
                    align=\"top\"></a> **One daemon stops another**\n\n\
                    **<picture><img src=\"https://x/amber-16.svg\" alt=\"\"></picture> Minor** · **Quick win**";
        assert_eq!(
            condensed(body),
            "[P1] **One daemon stops another**\n\n**Minor** · **Quick win**"
        );
    }

    #[test]
    fn the_cap_marks_the_cut_and_keeps_suggestions() {
        let mut body: String = (1..=40).map(|i| format!("line {i}\n")).collect();
        body.push_str("```suggestion\nfixed\n```\n");
        let c = condense(&body, 30);
        let lines: Vec<&str> = c.body.lines().collect();
        assert_eq!(lines[29], "line 30");
        assert_eq!(lines[30], "... 10 more lines (--full shows all)");
        assert_eq!(&lines[31..], ["```suggestion", "fixed", "```"]);
        assert!(c.truncated);
        assert_eq!(condense(&body, 0).body.lines().count(), 43);
    }

    #[test]
    fn a_code_block_cut_short_is_closed() {
        let body = format!("intro\n```rust\n{}```\nafter", "x\n".repeat(10));
        let c = condense(&body, 5);
        assert_eq!(
            c.body,
            "intro\n```rust\nx\nx\nx\n```\n... 9 more lines (--full shows all)"
        );
    }

    #[test]
    fn short_plain_bodies_are_unchanged() {
        let c = condense("good catch, is this also true\r\nfor the archive?", 30);
        assert_eq!(c.body, "good catch, is this also true\nfor the archive?");
        assert!(!c.truncated);
    }

    #[test]
    fn unclosed_markup_is_left_alone() {
        assert_eq!(condensed("a <!-- b"), "a <!-- b");
        assert_eq!(
            condensed("<details><summary>s</summary>"),
            "<details><summary>s</summary>"
        );
    }
}

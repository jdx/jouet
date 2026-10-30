//! Command shapes: a command reduced to its programs, subcommands and flags,
//! with every value replaced by a placeholder, such as
//! `gh pr checks <n> --repo <v> --watch | tail -<n>`.
//!
//! A shape is what a report may show of a command: it says what was run
//! without the paths, names, messages or scripts it was run with. Heredoc
//! bodies are dropped, and quoted or substituted words become `<s>`.

/// The lexer's tokens.
#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Word(Word),
    /// `|`, `|&`, `||`, `&&`, `;`, `;;`, `&`, a newline, `(` or `)`.
    Op(&'static str),
    /// `>`, `>>`, `<`, `2>`, `&>`, `2>&1`, `>&2`, `<<<`, ...
    Redirect(String),
    /// `<<DELIM`, whose body the lexer has skipped.
    Heredoc,
}

#[derive(Debug, Clone, PartialEq, Default)]
struct Word {
    /// The word with its quotes removed.
    text: String,
    /// Some part of it was quoted.
    quoted: bool,
    /// It holds an expansion or a command substitution.
    dynamic: bool,
}

#[derive(Default)]
struct WordBuf {
    text: Vec<u8>,
    quoted: bool,
    dynamic: bool,
}

/// Splits a shell command into words and operators, the way a POSIX shell
/// would, without expanding anything. Unbalanced quotes run to the end.
fn lex(src: &str) -> Vec<Tok> {
    let b = src.as_bytes();
    let mut toks = Vec::new();
    let mut cur: Option<WordBuf> = None;
    let mut heredocs: Vec<(String, bool)> = Vec::new();
    let end_word = |cur: &mut Option<WordBuf>, toks: &mut Vec<Tok>| {
        if let Some(w) = cur.take() {
            toks.push(Tok::Word(Word {
                text: String::from_utf8_lossy(&w.text).into_owned(),
                quoted: w.quoted,
                dynamic: w.dynamic,
            }));
        }
    };
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        let next = b.get(i + 1).copied();
        match c {
            b' ' | b'\t' | b'\r' => {
                end_word(&mut cur, &mut toks);
                i += 1;
            }
            b'\n' => {
                end_word(&mut cur, &mut toks);
                toks.push(Tok::Op("\n"));
                i += 1;
                if !heredocs.is_empty() {
                    i = skip_heredoc_bodies(b, i, &mut heredocs);
                }
            }
            b'\\' => {
                match next {
                    Some(b'\n') => {}
                    Some(n) => cur.get_or_insert_default().text.push(n),
                    None => {}
                }
                i += 2;
            }
            b'\'' => {
                let end = find(b, i + 1, b'\'');
                let w = cur.get_or_insert_default();
                w.text.extend_from_slice(&b[i + 1..end]);
                w.quoted = true;
                i = end + 1;
            }
            b'"' => {
                let w = cur.get_or_insert_default();
                w.quoted = true;
                i = lex_double(b, i + 1, w);
            }
            b'`' => {
                let end = find(b, i + 1, b'`');
                let w = cur.get_or_insert_default();
                w.text.extend_from_slice(&b[i..(end + 1).min(b.len())]);
                w.dynamic = true;
                i = end + 1;
            }
            b'$' if next == Some(b'(') => {
                let end = matching_paren(b, i + 1);
                let w = cur.get_or_insert_default();
                w.text.extend_from_slice(&b[i..end]);
                w.dynamic = true;
                i = end;
            }
            b'$' => {
                let w = cur.get_or_insert_default();
                w.text.push(c);
                w.dynamic = true;
                i += 1;
            }
            b'#' if cur.is_none() => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'|' => {
                end_word(&mut cur, &mut toks);
                let (op, n) = match next {
                    Some(b'|') => ("||", 2),
                    Some(b'&') => ("|&", 2),
                    _ => ("|", 1),
                };
                toks.push(Tok::Op(op));
                i += n;
            }
            b'&' if next == Some(b'>') => {
                end_word(&mut cur, &mut toks);
                let mut op = String::from("&>");
                i += 2;
                if b.get(i) == Some(&b'>') {
                    op.push('>');
                    i += 1;
                }
                toks.push(Tok::Redirect(op));
            }
            b'&' => {
                end_word(&mut cur, &mut toks);
                let (op, n) = if next == Some(b'&') {
                    ("&&", 2)
                } else {
                    ("&", 1)
                };
                toks.push(Tok::Op(op));
                i += n;
            }
            b';' => {
                end_word(&mut cur, &mut toks);
                let (op, n) = if next == Some(b';') {
                    (";;", 2)
                } else {
                    (";", 1)
                };
                toks.push(Tok::Op(op));
                i += n;
            }
            b'(' | b')' if cur.is_none() || c == b')' => {
                end_word(&mut cur, &mut toks);
                toks.push(Tok::Op(if c == b'(' { "(" } else { ")" }));
                i += 1;
            }
            b'>' | b'<' => {
                // A file descriptor written right before the operator, as in `2>`.
                let fd = match &cur {
                    Some(w)
                        if !w.quoted
                            && !w.dynamic
                            && !w.text.is_empty()
                            && w.text.iter().all(u8::is_ascii_digit) =>
                    {
                        cur.take()
                            .map(|w| String::from_utf8_lossy(&w.text).into_owned())
                    }
                    _ => None,
                };
                end_word(&mut cur, &mut toks);
                if c == b'<' && next == Some(b'<') {
                    if b.get(i + 2) == Some(&b'<') {
                        toks.push(Tok::Redirect("<<<".into()));
                        i += 3;
                        continue;
                    }
                    i += 2;
                    let strip = b.get(i) == Some(&b'-');
                    if strip {
                        i += 1;
                    }
                    while i < b.len() && matches!(b[i], b' ' | b'\t') {
                        i += 1;
                    }
                    let mut delim = Vec::new();
                    while i < b.len() && !b" \t\n;|&<>()".contains(&b[i]) {
                        if !matches!(b[i], b'\'' | b'"' | b'\\') {
                            delim.push(b[i]);
                        }
                        i += 1;
                    }
                    heredocs.push((String::from_utf8_lossy(&delim).into_owned(), strip));
                    toks.push(Tok::Heredoc);
                    continue;
                }
                let mut op = fd.unwrap_or_default();
                op.push(char::from(c));
                i += 1;
                if b.get(i) == Some(&c) {
                    op.push(char::from(c));
                    i += 1;
                }
                if b.get(i) == Some(&b'&') {
                    op.push('&');
                    i += 1;
                    while let Some(&d) = b.get(i) {
                        if d.is_ascii_digit() || d == b'-' {
                            op.push(char::from(d));
                            i += 1;
                        } else {
                            break;
                        }
                    }
                } else if b.get(i) == Some(&b'|') {
                    op.push('|');
                    i += 1;
                }
                toks.push(Tok::Redirect(op));
            }
            _ => {
                cur.get_or_insert_default().text.push(c);
                i += 1;
            }
        }
    }
    end_word(&mut cur, &mut toks);
    toks
}

/// The index of the next `quote` at or after `from`, or the end.
fn find(b: &[u8], from: usize, quote: u8) -> usize {
    let mut i = from;
    while i < b.len() {
        if b[i] == b'\\' && quote != b'\'' {
            i += 2;
            continue;
        }
        if b[i] == quote {
            return i;
        }
        i += 1;
    }
    b.len()
}

/// Given the index of a `(`, the index just past its matching `)`.
fn matching_paren(b: &[u8], open: usize) -> usize {
    let mut depth = 0;
    let mut i = open;
    while i < b.len() {
        match b[i] {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return i + 1;
                }
            }
            b'\'' | b'"' | b'`' => i = find(b, i + 1, b[i]),
            b'\\' => i += 1,
            _ => {}
        }
        i += 1;
    }
    b.len()
}

/// Reads a double-quoted string starting after its `"`, returning the index
/// just past the closing quote.
fn lex_double(b: &[u8], from: usize, w: &mut WordBuf) -> usize {
    let mut i = from;
    while i < b.len() {
        match b[i] {
            b'"' => return i + 1,
            b'\\' => {
                if let Some(&n) = b.get(i + 1) {
                    if !matches!(n, b'$' | b'`' | b'"' | b'\\' | b'\n') {
                        w.text.push(b'\\');
                    }
                    if n != b'\n' {
                        w.text.push(n);
                    }
                }
                i += 2;
            }
            b'$' if b.get(i + 1) == Some(&b'(') => {
                let end = matching_paren(b, i + 1);
                w.text.extend_from_slice(&b[i..end]);
                w.dynamic = true;
                i = end;
            }
            b'`' => {
                let end = find(b, i + 1, b'`');
                w.text.extend_from_slice(&b[i..(end + 1).min(b.len())]);
                w.dynamic = true;
                i = end + 1;
            }
            c => {
                if c == b'$' {
                    w.dynamic = true;
                }
                w.text.push(c);
                i += 1;
            }
        }
    }
    b.len()
}

/// Skips the bodies of the heredocs started on the line just ended, in order.
fn skip_heredoc_bodies(b: &[u8], mut i: usize, heredocs: &mut Vec<(String, bool)>) -> usize {
    for (delim, strip) in heredocs.drain(..) {
        loop {
            if i >= b.len() {
                return i;
            }
            let end = b[i..]
                .iter()
                .position(|&c| c == b'\n')
                .map_or(b.len(), |p| i + p);
            let mut line = &b[i..end];
            if strip {
                while let [b'\t', rest @ ..] = line {
                    line = rest;
                }
            }
            i = (end + 1).min(b.len());
            if line.trim_ascii_end() == delim.as_bytes() {
                break;
            }
        }
    }
    i
}

/// A command with every value replaced: `gh pr checks 15002 --repo jdx/mise`
/// becomes `gh pr checks <n> --repo <v>`. Programs keep only their file
/// name; long scripts are cut at their 8th command with a marker saying how
/// many more there were.
pub fn shape(command: &str) -> String {
    const MAX_COMMANDS: usize = 8;
    let mut out = String::new();
    let mut shown = 0;
    let mut hidden = 0;
    for (simple, op) in simple_commands(&lex(command)) {
        if simple.is_empty() && matches!(op, Some(";" | "\n") | None) {
            continue;
        }
        if shown == MAX_COMMANDS {
            if !simple.is_empty() {
                hidden += 1;
            }
            continue;
        }
        let (text, _) = shape_simple(&simple);
        out.push_str(&text);
        if !simple.is_empty() {
            shown += 1;
        }
        match op {
            Some(";" | "\n") => out.push_str("; "),
            Some("(") => out.push('('),
            Some(")") => {
                out.truncate(out.trim_end().len());
                out.push_str(") ");
            }
            Some("&") => out.push_str(" & "),
            Some(op) => {
                out.push(' ');
                out.push_str(op);
                out.push(' ');
            }
            None => {}
        }
    }
    let mut out = out.trim_end().trim_end_matches(';').trim_end().to_string();
    if hidden > 0 {
        out.push_str(&format!(" … +{hidden} more commands"));
    }
    out
}

/// The program each simple command runs, after assignments and wrappers
/// such as `env` and `timeout`.
pub(crate) fn programs(command: &str) -> Vec<String> {
    simple_commands(&lex(command))
        .into_iter()
        .filter_map(|(simple, _)| shape_simple(&simple).1)
        .collect()
}

/// Splits tokens into simple commands, each with the operator after it.
fn simple_commands(toks: &[Tok]) -> Vec<(Vec<&Tok>, Option<&'static str>)> {
    let mut out = Vec::new();
    let mut cur = Vec::new();
    for tok in toks {
        if let Tok::Op(op) = tok {
            out.push((std::mem::take(&mut cur), Some(*op)));
        } else {
            cur.push(tok);
        }
    }
    if !cur.is_empty() {
        out.push((cur, None));
    }
    out
}

/// Words that start or end a compound command, shown as they are.
const KEYWORDS: &[&str] = &[
    "if", "then", "else", "elif", "fi", "do", "done", "while", "until", "case", "esac", "!", "{",
    "}", "function",
];

/// Programs that run the program named after their own options.
const WRAPPERS: &[&str] = &[
    "sudo", "nohup", "command", "exec", "time", "nice", "builtin", "npx", "uvx", "bunx", "pnpx",
];

/// Shapes one simple command, returning the shape and the program it runs.
fn shape_simple(items: &[&Tok]) -> (String, Option<String>) {
    let mut parts: Vec<String> = Vec::new();
    let mut i = 0;
    let word = |i: usize| match items.get(i) {
        Some(Tok::Word(w)) => Some(w),
        _ => None,
    };

    // Keywords, then `for NAME in VALUES`.
    while let Some(kw) = word(i).and_then(plain).filter(|t| KEYWORDS.contains(t)) {
        parts.push(kw.to_string());
        i += 1;
    }
    if word(i).and_then(plain) == Some("for") {
        parts.push("for <v>".into());
        i += 2;
        if word(i).and_then(plain) == Some("in") {
            parts.push("in".into());
            i += 1;
        }
        rest_args(items, i, &mut parts);
        return (finish(parts), None);
    }
    // Leading assignments keep their names, so `GH_TOKEN= gh` stays visible.
    while let Some(w) = word(i) {
        let Some((name, value)) = w.text.split_once('=') else {
            break;
        };
        if w.text.starts_with('=') || !is_name(name) {
            break;
        }
        parts.push(if value.is_empty() && !w.dynamic {
            format!("{name}=")
        } else {
            format!("{name}=<v>")
        });
        i += 1;
    }
    // Wrappers, then the program.
    let mut program = None;
    while let Some(w) = word(i) {
        let Some(text) = plain(w) else {
            parts.push("<s>".into());
            i += 1;
            break;
        };
        let name = basename(text);
        i += 1;
        if WRAPPERS.contains(&name) {
            parts.push(name.to_string());
            i = flags(items, i, &mut parts);
            continue;
        }
        match name {
            "env" => {
                parts.push("env".into());
                while let Some(w) = word(i) {
                    if w.text == "-u" || w.text == "--unset" {
                        parts.push(w.text.clone());
                        if let Some(var) = word(i + 1) {
                            parts.push(if is_name(&var.text) {
                                var.text.clone()
                            } else {
                                "<v>".into()
                            });
                        }
                        i += 2;
                    } else if w.text.starts_with('-') {
                        parts.push(flag(&w.text));
                        i += 1;
                    } else if let Some((name, value)) = w.text.split_once('=')
                        && is_name(name)
                    {
                        parts.push(if value.is_empty() {
                            format!("{name}=")
                        } else {
                            format!("{name}=<v>")
                        });
                        i += 1;
                    } else {
                        break;
                    }
                }
                continue;
            }
            "timeout" => {
                parts.push("timeout".into());
                i = flags(items, i, &mut parts);
                if word(i).is_some() {
                    parts.push("<v>".into());
                    i += 1;
                }
                continue;
            }
            "xargs" => {
                parts.push("xargs".into());
                i = flags(items, i, &mut parts);
                continue;
            }
            _ => {}
        }
        parts.push(name.to_string());
        program = Some(name.to_string());
        i = subcommands(name, items, i, &mut parts, &mut program);
        break;
    }
    rest_args(items, i, &mut parts);
    (finish(parts), program)
}

/// A word's text, unless any of it was quoted or substituted.
fn plain(w: &Word) -> Option<&str> {
    (!w.quoted && !w.dynamic).then_some(w.text.as_str())
}

/// Joins the parts, collapsing runs of one placeholder into `<p>…`.
fn finish(parts: Vec<String>) -> String {
    let mut out: Vec<String> = Vec::with_capacity(parts.len());
    for part in parts {
        if let Some(last) = out.last_mut() {
            let base = last.trim_end_matches('…');
            if part.starts_with('<') && part.ends_with('>') && base == part {
                if !last.ends_with('…') {
                    last.push('…');
                }
                continue;
            }
        }
        out.push(part);
    }
    out.join(" ")
}

/// Leading flags of a wrapper, each followed by a value when it is not a
/// `--long=value` flag, returning where the wrapped command starts.
fn flags(items: &[&Tok], mut i: usize, parts: &mut Vec<String>) -> usize {
    while let Some(Tok::Word(w)) = items.get(i) {
        if !w.text.starts_with('-') || w.quoted {
            break;
        }
        parts.push(flag(&w.text));
        i += 1;
        if w.text == "--" {
            break;
        }
        // Short flags that take a value: `-u USER`, `-n N`, `-I {}`, `-s SIG`.
        if matches!(
            w.text.as_str(),
            "-u" | "-n" | "-I" | "-P" | "-s" | "-k" | "-g" | "-p" | "-L" | "-d"
        ) && items.get(i).is_some()
        {
            parts.push("<v>".into());
            i += 1;
        }
    }
    i
}

/// Keeps the subcommands of programs that have them, returning where the
/// remaining arguments start.
/// `mise x TOOL -- CMD` sets `runs` to the program CMD runs.
fn subcommands(
    program: &str,
    items: &[&Tok],
    mut i: usize,
    parts: &mut Vec<String>,
    runs: &mut Option<String>,
) -> usize {
    let ident = |i: usize| match items.get(i) {
        Some(Tok::Word(w)) if !w.quoted && !w.dynamic && is_ident(&w.text) => Some(w.text.clone()),
        _ => None,
    };
    let (nested, keep_name): (&[&str], &[&str]) = match program {
        "git" => {
            // Global options before the subcommand.
            while let Some(Tok::Word(w)) = items.get(i) {
                match w.text.as_str() {
                    "-C" | "-c" => {
                        parts.push(w.text.clone());
                        parts.push(if w.text == "-C" { "<path>" } else { "<v>" }.into());
                        i += 2;
                    }
                    t if t.starts_with("--") && !w.quoted => {
                        parts.push(flag(t));
                        i += 1;
                    }
                    _ => break,
                }
            }
            (
                &[
                    "stash",
                    "worktree",
                    "remote",
                    "submodule",
                    "bisect",
                    "notes",
                    "lfs",
                ],
                &[],
            )
        }
        "gh" => {
            if let Some(Tok::Word(w)) = items.get(i)
                && (w.text == "-R" || w.text == "--repo")
            {
                parts.push(w.text.clone());
                parts.push("<v>".into());
                i += 2;
            }
            if ident(i).as_deref() == Some("api") {
                parts.push("api".into());
                i += 1;
                i = flags_until_positional(items, i, parts);
                if let Some(Tok::Word(w)) = items.get(i) {
                    parts.push(
                        if w.text == "graphql" {
                            "graphql"
                        } else {
                            "<path>"
                        }
                        .into(),
                    );
                    i += 1;
                }
                return i;
            }
            (
                &[
                    "pr",
                    "run",
                    "issue",
                    "repo",
                    "release",
                    "workflow",
                    "search",
                    "stack",
                    "auth",
                    "label",
                    "gist",
                    "secret",
                    "variable",
                    "cache",
                    "extension",
                    "ruleset",
                    "project",
                    "codespace",
                ],
                &[],
            )
        }
        "cargo" => {
            if let Some(Tok::Word(w)) = items.get(i)
                && w.text.starts_with('+')
            {
                parts.push("+<v>".into());
                i += 1;
            }
            (&["nextest", "insta", "deny", "llvm-cov"], &[])
        }
        "mise" => {
            if let Some(sub) = ident(i)
                && matches!(sub.as_str(), "x" | "exec")
            {
                parts.push(sub);
                i += 1;
                while let Some(Tok::Word(w)) = items.get(i) {
                    i += 1;
                    if w.text == "--" {
                        parts.push("--".into());
                        let rest: Vec<&Tok> = items[i..].to_vec();
                        let (inner, inner_program) = shape_simple(&rest);
                        parts.push(inner);
                        *runs = inner_program;
                        return items.len();
                    }
                    parts.push(if w.text.starts_with('-') {
                        flag(&w.text)
                    } else {
                        "<v>".into()
                    });
                }
                return i;
            }
            (&["tasks", "settings", "plugins", "cache"], &["run", "r"])
        }
        "npm" | "pnpm" | "yarn" | "bun" | "aube" => (&[], &["run", "exec"]),
        "docker" | "podman" => (
            &[
                "compose",
                "container",
                "image",
                "system",
                "buildx",
                "volume",
                "network",
            ],
            &[],
        ),
        "jouet" => (&["pr", "wait", "edit", "threads", "rule"], &[]),
        "rustup" => (&["toolchain", "target", "component"], &[]),
        "go" => (&["mod"], &[]),
        "python" | "python3" => {
            // `python3 -m pytest`: the module is the program that runs.
            if let Some(Tok::Word(w)) = items.get(i)
                && w.text == "-m"
                && let Some(module) = ident(i + 1)
            {
                parts.push("-m".into());
                parts.push(module);
                return i + 2;
            }
            return i;
        }
        "kubectl" | "hk" | "mbx" | "uv" | "pip" | "pip3" | "brew" | "apt" | "apt-get" | "dnf"
        | "systemctl" | "entire" | "gt" | "make" | "just" | "pitchfork" | "usage"
        | "communique" | "deno" => (&[], &[]),
        _ => return i,
    };
    let Some(sub) = ident(i) else {
        return i;
    };
    parts.push(sub.clone());
    i += 1;
    if nested.contains(&sub.as_str()) {
        if let Some(sub2) = ident(i) {
            parts.push(sub2.clone());
            i += 1;
            // `jouet pr threads reply`, `gh stack submit`.
            if program == "jouet"
                && let Some(sub3) = ident(i)
                && matches!(sub3.as_str(), "reply" | "resolve" | "unresolve" | "apply")
            {
                parts.push(sub3);
                i += 1;
            }
        }
    } else if keep_name.contains(&sub.as_str()) {
        // A task or script name: `mise run test:e2e`, `npm run build`.
        i = flags_until_positional(items, i, parts);
        if let Some(name) = ident(i) {
            parts.push(name);
            i += 1;
        }
    }
    i
}

/// Flags before the first positional argument.
fn flags_until_positional(items: &[&Tok], mut i: usize, parts: &mut Vec<String>) -> usize {
    while let Some(Tok::Word(w)) = items.get(i) {
        if !w.text.starts_with('-') || w.quoted || w.text == "--" {
            break;
        }
        parts.push(flag(&w.text));
        i += 1;
    }
    i
}

/// The remaining arguments: flags kept, values replaced.
fn rest_args(items: &[&Tok], mut i: usize, parts: &mut Vec<String>) {
    let mut after_dashes = false;
    while let Some(tok) = items.get(i) {
        i += 1;
        match tok {
            Tok::Word(w) => {
                if w.text == "--" && !w.quoted {
                    after_dashes = true;
                    parts.push("--".into());
                } else if matches!(w.text.as_str(), "-" | "]" | "]]") && !w.quoted {
                    parts.push(w.text.clone());
                } else if !after_dashes && !w.quoted && w.text.len() > 1 && w.text.starts_with('-')
                {
                    parts.push(flag(&w.text));
                } else {
                    parts.push(placeholder(w));
                }
            }
            Tok::Redirect(op) => {
                parts.push(op.clone());
                // `2>&1` names its target; `> file` takes the next word.
                let takes_target = !op.contains('&') || op.starts_with('&');
                if takes_target && let Some(Tok::Word(target)) = items.get(i) {
                    parts.push(if target.text == "/dev/null" {
                        "/dev/null".into()
                    } else if op == "<<<" {
                        "<s>".into()
                    } else {
                        "<path>".into()
                    });
                    i += 1;
                }
            }
            Tok::Heredoc => parts.push("<<EOF".into()),
            Tok::Op(_) => {}
        }
    }
}

/// A flag with its value replaced: `--repo=jdx/mise` is `--repo=<v>`,
/// `-n5` is `-n<n>` and `-50` is `-<n>`.
fn flag(text: &str) -> String {
    if let Some((name, _)) = text.split_once('=') {
        return format!("{name}=<v>");
    }
    let body = text.trim_start_matches('-');
    let dashes = &text[..text.len() - body.len()];
    let letters = body.trim_end_matches(|c: char| c.is_ascii_digit());
    if letters.len() == body.len() {
        if body
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            text.to_string()
        } else {
            format!("{dashes}<v>")
        }
    } else {
        format!("{dashes}{letters}<n>")
    }
}

/// The placeholder for a value.
fn placeholder(w: &Word) -> String {
    let t = w.text.as_str();
    if w.quoted || w.dynamic || t.contains(char::is_whitespace) {
        "<s>"
    } else if !t.is_empty() && t.bytes().all(|c| c.is_ascii_digit()) {
        "<n>"
    } else if t.contains('/')
        || t.starts_with('.')
        || t.starts_with('~')
        || t.rsplit_once('.').is_some_and(|(stem, ext)| {
            !stem.is_empty()
                && (1..=5).contains(&ext.len())
                && ext.bytes().all(|c| c.is_ascii_alphanumeric())
        })
    {
        "<path>"
    } else {
        "<v>"
    }
    .into()
}

fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// A shell variable name.
fn is_name(s: &str) -> bool {
    let mut chars = s.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// A word that reads as a subcommand or task name rather than a value.
fn is_ident(s: &str) -> bool {
    let mut chars = s.chars();
    s.len() <= 40
        && chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "-_:".contains(c))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_programs_subcommands_and_flags() {
        assert_eq!(
            shape("gh pr checks 15002 --repo jdx/mise --watch"),
            "gh pr checks <n> --repo <path> --watch"
        );
        assert_eq!(
            shape("git -C ~/src/x status --short"),
            "git -C <path> status --short"
        );
        assert_eq!(shape("git stash pop"), "git stash pop");
        assert_eq!(shape("git checkout main"), "git checkout <v>");
        assert_eq!(
            shape("cargo +nightly test -p jouet-audit"),
            "cargo +<v> test -p <v>"
        );
        assert_eq!(
            shape("mise run test:e2e foo.bats"),
            "mise run test:e2e <path>"
        );
        assert_eq!(
            shape("/usr/local/bin/jouet pr threads reply 12 --body hi"),
            "jouet pr threads reply <n> --body <v>"
        );
        assert_eq!(
            shape("gh api repos/jdx/mise/pulls/1/comments"),
            "gh api <path>"
        );
        assert_eq!(
            shape("gh api graphql -f query='{ viewer { login } }'"),
            "gh api graphql -f <s>"
        );
        assert_eq!(
            shape("python3 -m pytest -x tests"),
            "python3 -m pytest -x <v>"
        );
    }

    #[test]
    fn replaces_values_and_collapses_runs() {
        assert_eq!(shape("sed -n '1,20p' src/main.rs"), "sed -n <s> <path>");
        assert_eq!(shape("cat a.rs b.rs c.rs"), "cat <path>…");
        assert_eq!(shape("head -n5 x.txt"), "head -n<n> <path>");
        assert_eq!(shape("tail -50 log"), "tail -<n> <v>");
        assert_eq!(shape("rg --type=rust 'fn main'"), "rg --type=<v> <s>");
        assert_eq!(shape("echo $(date) \"$HOME\""), "echo <s>…");
    }

    #[test]
    fn keeps_operators_redirects_and_assignments() {
        assert_eq!(
            shape("cargo test 2>&1 | head -50"),
            "cargo test 2>&1 | head -<n>"
        );
        assert_eq!(
            shape("GH_TOKEN= GITHUB_TOKEN= gh pr view 1 --json state > out.json"),
            "GH_TOKEN= GITHUB_TOKEN= gh pr view <n> --json <v> > <path>"
        );
        assert_eq!(shape("RUST_LOG=debug cargo run"), "RUST_LOG=<v> cargo run");
        assert_eq!(
            shape("env -u GH_TOKEN timeout 30 gh run watch 5 >/dev/null"),
            "env -u GH_TOKEN timeout <v> gh run watch <n> > /dev/null"
        );
        assert_eq!(
            shape("cd x && make test; echo ok"),
            "cd <v> && make test; echo <v>"
        );
    }

    #[test]
    fn drops_heredoc_bodies_and_loops_keep_their_keywords() {
        let cmd = "python3 - <<'EOF'\nimport os\nprint(open('secret').read())\nEOF\necho done";
        assert_eq!(shape(cmd), "python3 - <<EOF; echo <v>");
        assert_eq!(
            shape("cat > f.txt <<-EOF\n\tbody\n\tEOF"),
            "cat > <path> <<EOF"
        );
        assert_eq!(
            shape("while ! gh pr checks 3; do sleep 30; done"),
            "while ! gh pr checks <n>; do sleep <n>; done"
        );
        assert_eq!(
            shape("for p in 1 2 3; do gh pr view $p; done"),
            "for <v> in <n>…; do gh pr view <s>; done"
        );
        assert_eq!(
            shape("mise x hk -- hk check --all"),
            "mise x <v> -- hk check --all"
        );
        assert_eq!(shape("# just a comment"), "");
        assert_eq!(
            shape("until [ -s out.txt ]; do sleep 1; done"),
            "until [ -s <path> ]; do sleep <n>; done"
        );
    }

    #[test]
    fn marks_what_it_leaves_out() {
        let cmd = (1..=10)
            .map(|n| format!("echo {n}"))
            .collect::<Vec<_>>()
            .join("; ");
        assert_eq!(
            shape(&cmd),
            "echo <n>; echo <n>; echo <n>; echo <n>; echo <n>; echo <n>; echo <n>; echo <n> … +2 more commands"
        );
    }

    #[test]
    fn finds_the_programs_wrappers_run() {
        assert_eq!(
            programs("FOO=1 sudo -u root timeout 5 cargo test | tee log"),
            ["cargo", "tee"]
        );
    }
}

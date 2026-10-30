//! The structure of a compound command: its loops, conditionals, `case`
//! branches, subshells, groups and functions, around the simple commands
//! inside them. A shaper with its own placeholders, such as jouet's hook
//! ledger, keeps the structure and drops every value: a loop's list, a case
//! word and its patterns, a function's name.

use crate::shape::{Tok, lex};

/// One piece of a script's structure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Part {
    /// A reserved word or bracket: `if`, `then`, `do`, `done`, `{`, `(`,
    /// `;;`, `!` and the rest.
    Keyword(&'static str),
    /// `for NAME in WORDS` (or `select`), its name and words dropped;
    /// `list` is false for `for NAME` alone.
    For { select: bool, list: bool },
    /// `case WORD in`, its word dropped.
    CaseIn,
    /// A `case` branch's pattern, `PATTERN)`.
    Pattern,
    /// A function's header, `NAME()` or `function NAME`, its name dropped.
    Function,
    /// A simple command's words, with quotes removed and redirections and
    /// here-documents dropped.
    Command(Vec<String>),
    /// The operator after a command: `;` (or a newline), `&&`, `||`, `|`,
    /// `|&` or `&`.
    Op(&'static str),
}

/// Reserved words that open or close a compound command, as [`Part`]
/// keeps them.
pub(crate) const KEYWORDS: &[&str] = &[
    "if", "then", "elif", "else", "fi", "while", "until", "do", "done", "esac", "{", "}", "!",
];

/// A script's structure, or `None` when it has no compound command, so a
/// shaper can keep the flat form it has always given simple commands,
/// pipelines and `&&` chains.
pub fn structure(script: &str) -> Option<Vec<Part>> {
    let toks = lex(script);
    let mut parts = Vec::new();
    let mut compound = false;
    // Open `case` commands, and whether the next words are a pattern.
    let mut cases = 0usize;
    let mut pattern_next = false;
    let mut i = 0;
    while i < toks.len() {
        // One simple command: the tokens up to the next operator.
        let end = toks[i..]
            .iter()
            .position(|t| matches!(t, Tok::Op(_)))
            .map_or(toks.len(), |p| i + p);
        let op = match toks.get(end) {
            Some(Tok::Op(op)) => Some(*op),
            _ => None,
        };
        let items = &toks[i..end];
        i = end + 1;
        let words: Vec<&str> = items
            .iter()
            .filter_map(|t| match t {
                Tok::Word(w) => Some(w.text.as_str()),
                _ => None,
            })
            .collect();

        if cases > 0 && pattern_next {
            if words.first() == Some(&"esac") {
                parts.push(Part::Keyword("esac"));
                cases -= 1;
                pattern_next = false;
                push_op(&mut parts, op);
                continue;
            }
            // `(PATTERN)`: the opening bracket is optional.
            if words.is_empty() && op == Some("(") {
                continue;
            }
            if op == Some(")") {
                parts.push(Part::Pattern);
                pattern_next = false;
                continue;
            }
            if words.is_empty() {
                push_op(&mut parts, op);
                continue;
            }
            pattern_next = false;
        }

        // `NAME() {` lexes as the word `NAME(` and a `)`; `NAME () {` as
        // a word, `(` and `)`.
        if let [name] = words[..]
            && items.len() == 1
        {
            let glued = name.ends_with('(') && op == Some(")");
            let spaced = op == Some("(") && matches!(toks.get(i), Some(Tok::Op(")")));
            if glued || spaced {
                parts.push(Part::Function);
                compound = true;
                i += usize::from(spaced);
                continue;
            }
        }

        let mut rest = items;
        while let Some(Tok::Word(w)) = rest.first() {
            let text = w.text.as_str();
            if let Some(kw) = KEYWORDS.iter().find(|k| **k == text) {
                parts.push(Part::Keyword(kw));
                compound = true;
                rest = &rest[1..];
                continue;
            }
            match text {
                "for" | "select" => {
                    let list = rest
                        .iter()
                        .any(|t| matches!(t, Tok::Word(w) if w.text == "in"));
                    parts.push(Part::For {
                        select: text == "select",
                        list,
                    });
                    rest = &[];
                }
                "case" => {
                    parts.push(Part::CaseIn);
                    cases += 1;
                    pattern_next = true;
                    rest = &[];
                }
                "function" => {
                    // `function NAME { BODY`: the body follows.
                    parts.push(Part::Function);
                    compound = true;
                    rest = rest.get(2..).unwrap_or_default();
                    continue;
                }
                _ => break,
            }
            compound = true;
            break;
        }
        let command = command_words(rest);
        if !command.is_empty() {
            parts.push(Part::Command(command));
        }
        match op {
            Some("(") => {
                parts.push(Part::Keyword("("));
                compound = true;
            }
            Some(")") => {
                parts.push(Part::Keyword(")"));
                compound = true;
            }
            Some(";;") => {
                parts.push(Part::Keyword(";;"));
                pattern_next = cases > 0;
            }
            op => push_op(&mut parts, op),
        }
    }
    compound.then_some(parts)
}

/// Adds the operator after a command. A separator where one adds nothing
/// (at the start, twice, or after a word that opens a body, such as `do`)
/// is left out, so line breaks do not change the structure.
/// Each simple command's words, flat or compound alike, in order, and
/// whether a pipe feeds it.
pub(crate) fn simple_words(script: &str) -> Vec<(Vec<String>, bool)> {
    let toks = lex(script);
    let mut out = Vec::new();
    let mut piped = false;
    for chunk in toks.split_inclusive(|t| matches!(t, Tok::Op(_))) {
        let words = command_words(chunk);
        if !words.is_empty() {
            out.push((words, piped));
        }
        piped = matches!(chunk.last(), Some(Tok::Op("|" | "|&")));
    }
    out
}

fn push_op(parts: &mut Vec<Part>, op: Option<&'static str>) {
    let op = match op {
        Some("\n" | ";") => ";",
        Some(op) => op,
        None => return,
    };
    let redundant = op == ";"
        && matches!(
            parts.last(),
            None | Some(
                Part::Op(_)
                    | Part::CaseIn
                    | Part::Pattern
                    | Part::Function
                    | Part::Keyword("then" | "else" | "do" | "{" | "(" | ";;" | "!")
            )
        );
    if !redundant {
        parts.push(Part::Op(op));
    }
}

/// A simple command's words, without redirections, their targets and
/// here-documents.
fn command_words(items: &[Tok]) -> Vec<String> {
    let mut out = Vec::new();
    let mut target = false;
    for tok in items {
        match tok {
            Tok::Word(w) => {
                if !std::mem::take(&mut target) {
                    out.push(w.text.clone());
                }
            }
            // `2>&1` and `>&-` name no file; `> out` and `<<< text` do.
            Tok::Redirect(r) => {
                target =
                    !(r.contains('&') && r.ends_with(|c: char| c.is_ascii_digit() || c == '-'));
            }
            Tok::Heredoc | Tok::Op(_) => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(script: &str) -> Vec<String> {
        structure(script)
            .expect("compound")
            .iter()
            .map(|p| match p {
                Part::Keyword(k) => (*k).to_string(),
                Part::For { list: true, .. } => "FOR-IN".into(),
                Part::For { list: false, .. } => "FOR".into(),
                Part::CaseIn => "CASE-IN".into(),
                Part::Pattern => "PAT)".into(),
                Part::Function => "FN()".into(),
                Part::Command(w) => format!("[{}]", w.join(" ")),
                Part::Op(op) => (*op).to_string(),
            })
            .collect()
    }

    #[test]
    fn flat_scripts_have_no_structure() {
        for flat in [
            "cargo test",
            "cd x && cargo test 2>&1 | tail -5",
            "echo done; echo if",
            "git commit -m 'for x in y; do z; done'",
            "cat <<EOF\nfor x in y; do\nEOF",
        ] {
            assert_eq!(structure(flat), None, "{flat}");
        }
    }

    #[test]
    fn loops_keep_their_bodies_and_drop_their_lists() {
        assert_eq!(
            kinds("cd repo && for n in 1 2 3; do gh pr view $n --json title > out.txt; done"),
            [
                "[cd repo]",
                "&&",
                "FOR-IN",
                ";",
                "do",
                "[gh pr view $n --json title]",
                ";",
                "done"
            ]
        );
        assert_eq!(
            kinds("while true; do sleep 5; done"),
            ["while", "[true]", ";", "do", "[sleep 5]", ";", "done"]
        );
    }

    #[test]
    fn conditionals_cases_subshells_and_functions() {
        assert_eq!(
            kinds("if [ -f x ]; then echo y; else echo z; fi"),
            [
                "if",
                "[[ -f x ]]",
                ";",
                "then",
                "[echo y]",
                ";",
                "else",
                "[echo z]",
                ";",
                "fi"
            ]
        );
        assert_eq!(
            kinds("case \"$e\" in\n  post-tool-use) jq . f;;\n  *) echo other;;\nesac"),
            [
                "CASE-IN",
                "PAT)",
                "[jq . f]",
                ";;",
                "PAT)",
                "[echo other]",
                ";;",
                "esac"
            ]
        );
        assert_eq!(
            kinds("(cd x && make) || echo failed"),
            ["(", "[cd x]", "&&", "[make]", ")", "||", "[echo failed]"]
        );
        assert_eq!(
            kinds("f() { ls; }; f"),
            ["FN()", "{", "[ls]", ";", "}", ";", "[f]"]
        );
        assert_eq!(kinds("function g { ls; }"), ["FN()", "{", "[ls]", ";", "}"]);
        assert_eq!(
            kinds("if ! cargo test\nthen\n  echo no\nfi"),
            [
                "if",
                "!",
                "[cargo test]",
                ";",
                "then",
                "[echo no]",
                ";",
                "fi"
            ]
        );
    }
}

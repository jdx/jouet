//! Telling a user's correction from an ordinary prompt: "no, use X",
//! "don't do that", "I said...", "instead...".
//!
//! Plain cue words, matched on word boundaries, in the order a person
//! writes them: a cue that opens the message counts for more than one in
//! the middle, where "no" or "instead" is often just part of a request. It
//! errs toward flagging: a reviewer reads each correction with its span,
//! and a missed one hides the strongest signal a session has.

use serde::Serialize;

/// What kind of correction a message is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// The user stopped the agent mid-turn.
    Interrupt,
    /// The user refused a tool call.
    Refusal,
    /// "stop", "hold on", "wait".
    Stop,
    /// "no", "don't", "that's wrong", "undo that".
    Negation,
    /// "use X instead", "rather than", "actually".
    Redirect,
    /// "I said", "again", "as I told you": an instruction not followed.
    Repeat,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Interrupt => "interrupt",
            Self::Refusal => "refusal",
            Self::Stop => "stop",
            Self::Negation => "negation",
            Self::Redirect => "redirect",
            Self::Repeat => "repeat",
        }
    }
}

/// Cues that count only when they open the message (after a greeting or
/// the user's name for the agent): a sentence that starts with "no" is a
/// correction; "no" later on rarely is.
const OPENING: &[(&str, Kind)] = &[
    ("stop", Kind::Stop),
    ("wait", Kind::Stop),
    ("hold on", Kind::Stop),
    ("hang on", Kind::Stop),
    ("no", Kind::Negation),
    ("nope", Kind::Negation),
    ("nah", Kind::Negation),
    ("not that", Kind::Negation),
    ("wrong", Kind::Negation),
    ("undo", Kind::Negation),
    ("revert", Kind::Negation),
    ("don't", Kind::Negation),
    ("dont", Kind::Negation),
    ("do not", Kind::Negation),
    ("never", Kind::Negation),
    ("actually", Kind::Redirect),
    ("instead", Kind::Redirect),
    ("again", Kind::Repeat),
];

/// Cues that count anywhere.
const ANYWHERE: &[(&str, Kind)] = &[
    ("that's wrong", Kind::Negation),
    ("thats wrong", Kind::Negation),
    ("that's not what", Kind::Negation),
    ("not what i", Kind::Negation),
    ("didn't ask", Kind::Negation),
    ("did not ask", Kind::Negation),
    ("don't do", Kind::Negation),
    ("stop doing", Kind::Stop),
    ("why did you", Kind::Negation),
    ("why are you", Kind::Negation),
    ("you shouldn't", Kind::Negation),
    ("you should not", Kind::Negation),
    ("you should have", Kind::Negation),
    ("you forgot", Kind::Repeat),
    ("you missed", Kind::Repeat),
    ("still broken", Kind::Repeat),
    ("still failing", Kind::Repeat),
    ("still not", Kind::Repeat),
    ("i said", Kind::Repeat),
    ("i told you", Kind::Repeat),
    ("as i said", Kind::Repeat),
    ("i already", Kind::Repeat),
    ("like i said", Kind::Repeat),
    ("instead", Kind::Redirect),
    ("rather than", Kind::Redirect),
    ("not like that", Kind::Redirect),
];

/// Openings that start with a cue word but are not corrections.
const NOT_CORRECTIONS: &[&str] = &[
    "no problem",
    "no worries",
    "no rush",
    "no need to",
    "no thanks",
    "wait for",
    "wait until",
    "wait till",
    "again, thanks",
];

/// Cues too common in questions to count in one: "no regressions?",
/// "could we use X instead?".
const WEAK: &[&str] = &[
    "no",
    "nope",
    "nah",
    "instead",
    "actually",
    "rather than",
    "again",
];

/// A message's correction kind and the cues that matched, or `None` for an
/// ordinary prompt. The strongest kind wins: a stop over a negation over a
/// repeat over a redirect. Only the user's own words count: pasted
/// content, quoted lines and HTML comments are left out.
pub fn classify(text: &str) -> Option<(Kind, Vec<&'static str>)> {
    let lower = normalize(&own_words(text));
    let opening = strip_greeting(&lower);
    let question = first_sentence(opening).ends_with('?');
    let mut cues: Vec<(&'static str, Kind)> = Vec::new();
    if !NOT_CORRECTIONS.iter().any(|n| opening.starts_with(n)) {
        cues.extend(
            OPENING
                .iter()
                .filter(|(cue, _)| starts_with_word(opening, cue))
                .copied(),
        );
    }
    for &(cue, kind) in ANYWHERE {
        if !cues.iter().any(|(c, _)| *c == cue) && contains_word(&lower, cue) {
            cues.push((cue, kind));
        }
    }
    if question {
        cues.retain(|(cue, _)| !WEAK.contains(cue));
    }
    let kind = cues.iter().map(|(_, k)| *k).min_by_key(|k| strength(*k))?;
    Some((kind, cues.into_iter().map(|(c, _)| c).collect()))
}

/// The first sentence, ending at `?`, `!`, or a `.` before a space (not
/// the one in `6.11.1`).
fn first_sentence(text: &str) -> &str {
    let b = text.as_bytes();
    let end = (0..b.len())
        .find(|&i| {
            matches!(b[i], b'?' | b'!')
                || b[i] == b'.' && b.get(i + 1).is_none_or(u8::is_ascii_whitespace)
        })
        .map_or(b.len(), |i| i + 1);
    &text[..end]
}

/// The text before any pasted content, without quoted lines (`> ...`)
/// or HTML comments.
fn own_words(text: &str) -> String {
    let text = text.split("<pasted_content").next().unwrap_or(text);
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("<!--") {
        out.push_str(&rest[..start]);
        rest = rest[start..]
            .find("-->")
            .map_or("", |end| &rest[start + end + 3..]);
    }
    out.push_str(rest);
    out.lines()
        .filter(|l| !l.trim_start().starts_with('>'))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Lower is stronger.
fn strength(kind: Kind) -> u8 {
    match kind {
        Kind::Interrupt | Kind::Refusal => 0,
        Kind::Stop => 1,
        Kind::Negation => 2,
        Kind::Repeat => 3,
        Kind::Redirect => 4,
    }
}

/// Lower case, curly apostrophes made straight, code spans and blocks
/// dropped (a quoted `--no-verify` is not a "no"), and whitespace
/// collapsed.
fn normalize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for block in text.split("```").step_by(2) {
        for prose in block.split('`').step_by(2) {
            out.push_str(prose);
            out.push(' ');
        }
    }
    out.replace(['\u{2019}', '\u{2018}'], "'")
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// The message after an opening such as "ok,", "hmm" or "claude,".
fn strip_greeting(text: &str) -> &str {
    let mut rest = text;
    for greeting in [
        "ok", "okay", "hmm", "um", "uh", "claude", "codex", "hey", "so",
    ] {
        if let Some(after) = rest.strip_prefix(greeting)
            && after.starts_with([',', ' ', '.', '!'])
        {
            rest = after.trim_start_matches([',', ' ', '.', '!']);
        }
    }
    rest
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '\'' || c == '_' || c == '-'
}

fn starts_with_word(text: &str, word: &str) -> bool {
    text.strip_prefix(word)
        .is_some_and(|rest| !rest.starts_with(is_word_char))
}

fn contains_word(text: &str, word: &str) -> bool {
    text.match_indices(word).any(|(at, _)| {
        let before = text[..at].chars().next_back();
        let after = text[at + word.len()..].chars().next();
        !before.is_some_and(is_word_char) && !after.is_some_and(is_word_char)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kind(text: &str) -> Option<Kind> {
        classify(text).map(|(k, _)| k)
    }

    #[test]
    fn corrections_are_found_by_their_cues() {
        for (text, expected) in [
            ("no, use jouet wait instead", Kind::Negation),
            ("No. Don't touch the vendored registry", Kind::Negation),
            ("stop, that's the wrong branch", Kind::Stop),
            ("ok, wait — you're editing main", Kind::Stop),
            ("that's not what I asked for", Kind::Negation),
            ("I said to stage explicit paths", Kind::Repeat),
            ("the tests are still failing", Kind::Repeat),
            ("use mise run test instead of cargo test", Kind::Redirect),
            ("Actually, put it in mise.local.toml", Kind::Redirect),
            ("why did you push to main?", Kind::Negation),
            ("don\u{2019}t open a draft", Kind::Negation),
            (
                "why did you add this? <pasted_content id=\"1\">x</pasted_content>",
                Kind::Negation,
            ),
            ("no, I said the other one?", Kind::Repeat),
        ] {
            assert_eq!(kind(text), Some(expected), "{text}");
        }
    }

    #[test]
    fn ordinary_prompts_are_not_corrections() {
        for text in [
            "make a pr to fix, post reply on discussion",
            "no problem, carry on with the rebase",
            "wait for CI to finish, then merge",
            "how does python behave with --no-binary?",
            "add a `--no-verify` check to the rule",
            "know what? let's ship it",
            "renovate the instance pool",
            "```\nno\n```\nsee above",
            "is there another term we could use instead of --local?",
            "ok so no regressions since 6.11.1 on main?",
            "<!-- no, rather than this --> thanks, ship it",
            "> no, that is wrong\nwhat do you make of this review?",
            "address this: <pasted_content id=\"1\">don't do that</pasted_content>",
        ] {
            assert_eq!(kind(text), None, "{text}");
        }
    }

    #[test]
    fn the_strongest_cue_decides_and_all_are_kept() {
        let (kind, cues) = classify("no, stop, I said use the other one instead").unwrap();
        assert_eq!(kind, Kind::Negation);
        assert_eq!(cues, ["no", "i said", "instead"]);
        let (kind, _) = classify("stop. I said use the other one").unwrap();
        assert_eq!(kind, Kind::Stop);
    }
}

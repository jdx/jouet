//! Just enough of the POSIX shell grammar to tell a command from a string.
//!
//! The hooks read the command an agent is about to run and must not mistake
//! `git commit -m "never git add -A"` for `git add -A`. This splits a script
//! into simple commands at unquoted operators, removes quotes, drops
//! redirections, and keeps here-document bodies apart from the words, which is
//! all a pattern needs. It never fails: text it cannot make sense of becomes
//! words, and the worst outcome is a missed match.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    /// `;` or a newline.
    Seq,
    And,
    Or,
    Pipe,
    Background,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Command {
    /// Words after quote removal, without leading reserved words (`do`,
    /// `then`, ...), variable assignments or redirections.
    pub words: Vec<String>,
    /// The bodies of the command's here-documents.
    pub heredocs: Vec<String>,
    /// The operator after the command, or `None` at the end of the script.
    pub op: Option<Op>,
}

/// A parsed script: its top-level commands, and the commands inside command
/// substitutions, which run too but whose exit status is not the script's.
#[derive(Debug, Default)]
pub struct Script {
    pub commands: Vec<Command>,
    pub nested: Vec<Command>,
}

impl Script {
    /// Every command the script runs: top-level, substituted, and the scripts
    /// given to `sh -c` and friends, recursively.
    pub fn all(&self) -> Vec<&Command> {
        self.commands.iter().chain(&self.nested).collect()
    }

    /// The scripts passed to `sh -c`, `bash -c` and the like, parsed.
    pub fn inner_scripts(&self) -> Vec<Script> {
        self.all()
            .into_iter()
            .filter_map(|c| shell_c_script(c.argv()))
            .map(parse)
            .collect()
    }
}

/// What a script exiting with `exit_code` says about each top-level command.
///
/// A failure belongs only to the first command of the last pipeline, so
/// `cargo test; echo done` failing says nothing about the tests. Success
/// belongs to every command in the final `&&` chain, because all of them ran
/// and succeeded. A command piped into `tail` gets whatever `tail` returned,
/// which errs toward a missed failure, never an invented one. A script that
/// ends in the background says nothing about anything.
pub fn statuses(commands: &[Command], exit_code: i32) -> Vec<Option<i32>> {
    let mut out = vec![None; commands.len()];
    let Some(last) = commands.last() else {
        return out;
    };
    if last.op == Some(Op::Background) {
        return out;
    }
    let mut start = commands.len() - 1;
    while start > 0 && matches!(commands[start - 1].op, Some(Op::And | Op::Pipe)) {
        start -= 1;
    }
    if exit_code == 0 {
        for status in &mut out[start..] {
            *status = Some(0);
        }
        return out;
    }
    let mut pipeline = commands.len() - 1;
    while pipeline > start && commands[pipeline - 1].op == Some(Op::Pipe) {
        pipeline -= 1;
    }
    out[pipeline] = Some(exit_code);
    out
}

pub fn parse(script: &str) -> Script {
    let mut parser = Parser {
        chars: script.chars().collect(),
        pos: 0,
        nested: Vec::new(),
    };
    let commands = parser.list(false);
    Script {
        commands,
        nested: parser.nested,
    }
}

const RESERVED: &[&str] = &[
    "!", "{", "}", "(", ")", "do", "done", "then", "else", "elif", "fi", "if", "while", "until",
    "esac", "time",
];

impl Command {
    /// The words with wrappers such as `env FOO=1`, `sudo`, `timeout 60`,
    /// `rtk` and `mise x tool --` removed, so `[0]` is the program that runs.
    pub fn argv(&self) -> &[String] {
        let mut w = &self.words[..];
        loop {
            let Some(first) = w.first() else { return w };
            let rest = &w[1..];
            w = match basename(first) {
                "env" => skip_options(rest, &["-u", "-C", "-S", "--unset", "--chdir"], true),
                "sudo" => skip_options(rest, &["-u", "-g", "-C", "-h", "-p"], false),
                "nice" => skip_options(rest, &["-n"], false),
                "timeout" => {
                    let rest = skip_options(rest, &["-s", "-k", "--signal", "--kill-after"], false);
                    rest.get(1..).unwrap_or_default()
                }
                "command" | "exec" | "nohup" | "time" | "rtk" => skip_options(rest, &[], false),
                "mise" if matches!(rest.first().map(String::as_str), Some("x" | "exec")) => {
                    match rest.iter().position(|a| a == "--") {
                        Some(i) => &rest[i + 1..],
                        None => return w,
                    }
                }
                _ => return w,
            };
        }
    }

    /// The program's basename: `/usr/bin/git` and `git.exe` are both `git`.
    pub fn program(&self) -> Option<&str> {
        self.argv().first().map(|p| basename(p))
    }
}

pub fn basename(word: &str) -> &str {
    let name = word.rsplit(['/', '\\']).next().unwrap_or(word);
    name.strip_suffix(".exe").unwrap_or(name)
}

/// Skips leading options, the values of the ones in `with_value`, and (for
/// `env`) `NAME=value` words.
fn skip_options<'a>(mut w: &'a [String], with_value: &[&str], assignments: bool) -> &'a [String] {
    while let Some(first) = w.first() {
        if first == "--" {
            return &w[1..];
        }
        if with_value.contains(&first.as_str()) {
            w = w.get(2..).unwrap_or_default();
        } else if first.starts_with('-') || (assignments && is_assignment(first)) {
            w = &w[1..];
        } else {
            break;
        }
    }
    w
}

fn is_assignment(word: &str) -> bool {
    match word.split_once('=') {
        Some((name, _)) => {
            let mut chars = name.chars();
            chars
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
                && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        }
        None => false,
    }
}

/// The script in `sh -c SCRIPT`, `bash -lc SCRIPT` and the like.
fn shell_c_script(argv: &[String]) -> Option<&str> {
    let program = basename(argv.first()?);
    if !matches!(program, "sh" | "bash" | "zsh" | "dash" | "ksh" | "fish") {
        return None;
    }
    let mut rest = argv[1..].iter();
    for arg in rest.by_ref() {
        if !arg.starts_with('-') {
            return None;
        }
        if arg[1..].contains('c') && !arg.starts_with("--") {
            break;
        }
    }
    rest.next().map(String::as_str)
}

struct Parser {
    chars: Vec<char>,
    pos: usize,
    nested: Vec<Command>,
}

/// A here-document waiting for the next newline: its delimiter, whether `<<-`
/// strips leading tabs, and which command it belongs to.
struct Pending {
    delimiter: String,
    strip_tabs: bool,
    owner: usize,
}

struct Builder {
    commands: Vec<Command>,
    current: Command,
    word: Option<String>,
    /// The next word is a redirection's target, not an argument.
    skip_word: bool,
    /// The next word is a here-document delimiter.
    heredoc: Option<bool>,
    pending: Vec<Pending>,
}

impl Builder {
    fn end_word(&mut self) {
        let Some(word) = self.word.take() else { return };
        if let Some(strip_tabs) = self.heredoc.take() {
            self.pending.push(Pending {
                delimiter: word,
                strip_tabs,
                owner: self.commands.len(),
            });
        } else if self.skip_word {
            self.skip_word = false;
        } else {
            self.current.words.push(word);
        }
    }

    fn end_command(&mut self, op: Option<Op>) {
        self.end_word();
        let mut command = std::mem::take(&mut self.current);
        let keep = command
            .words
            .iter()
            .position(|w| !RESERVED.contains(&w.as_str()) && !is_assignment(w))
            .unwrap_or(command.words.len());
        command.words.drain(..keep);
        command.op = op;
        if command.words.is_empty() && command.heredocs.is_empty() {
            // `do`, `done`, a blank line: carry the operator to the previous
            // command so `a && \n b` still chains.
            if let (Some(op), Some(last)) = (op, self.commands.last_mut())
                && last.op == Some(Op::Seq)
            {
                last.op = Some(op);
            }
            return;
        }
        self.commands.push(command);
    }

    fn word(&mut self) -> &mut String {
        self.word.get_or_insert_with(String::new)
    }
}

impl Parser {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    /// Parses commands up to the end, or up to the `)` closing a command
    /// substitution when `in_subst`.
    fn list(&mut self, in_subst: bool) -> Vec<Command> {
        let mut b = Builder {
            commands: Vec::new(),
            current: Command::default(),
            word: None,
            skip_word: false,
            heredoc: None,
            pending: Vec::new(),
        };
        while let Some(c) = self.peek() {
            self.pos += 1;
            match c {
                ' ' | '\t' | '\r' => b.end_word(),
                '\n' => {
                    b.end_word();
                    self.read_heredocs(&mut b);
                    b.end_command(Some(Op::Seq));
                }
                '#' if b.word.is_none() => {
                    while self.peek().is_some_and(|c| c != '\n') {
                        self.pos += 1;
                    }
                }
                '\\' => match self.peek() {
                    Some('\n') => self.pos += 1,
                    Some(next) => {
                        self.pos += 1;
                        b.word().push(next);
                    }
                    None => b.word().push('\\'),
                },
                '\'' => {
                    let word = b.word();
                    while let Some(c) = self.chars.get(self.pos).copied() {
                        self.pos += 1;
                        if c == '\'' {
                            break;
                        }
                        word.push(c);
                    }
                }
                '"' => self.double_quoted(&mut b),
                '`' => {
                    let text = self.backquoted();
                    b.word().push_str(&text);
                }
                '$' if self.peek() == Some('(') => {
                    let text = self.substitution();
                    b.word().push_str(&text);
                }
                ')' if in_subst => break,
                '(' | ')' => {
                    b.end_command(Some(Op::Seq));
                }
                ';' => {
                    if self.peek() == Some(';') {
                        self.pos += 1;
                    }
                    b.end_command(Some(Op::Seq));
                }
                '&' => match self.peek() {
                    Some('&') => {
                        self.pos += 1;
                        b.end_command(Some(Op::And));
                    }
                    Some('>') => {
                        b.end_word();
                        self.redirect(&mut b);
                    }
                    _ => b.end_command(Some(Op::Background)),
                },
                '|' => {
                    if self.peek() == Some('|') {
                        self.pos += 1;
                        b.end_command(Some(Op::Or));
                    } else {
                        if self.peek() == Some('&') {
                            self.pos += 1;
                        }
                        b.end_command(Some(Op::Pipe));
                    }
                }
                '<' | '>' => {
                    // `2>` and `10<`: an all-digit word before is the fd.
                    if b.word
                        .as_deref()
                        .is_some_and(|w| w.chars().all(|c| c.is_ascii_digit()))
                    {
                        b.word = None;
                    }
                    b.end_word();
                    self.pos -= 1;
                    self.redirect(&mut b);
                }
                c => b.word().push(c),
            }
        }
        b.end_word();
        self.read_heredocs(&mut b);
        b.end_command(None);
        b.commands
    }

    /// At `<`, `>` or `&>`: consumes the operator, and arranges for the target
    /// word to be dropped or read as a here-document delimiter.
    fn redirect(&mut self, b: &mut Builder) {
        let start = self.pos;
        while self
            .peek()
            .is_some_and(|c| matches!(c, '<' | '>' | '&' | '|' | '-'))
        {
            // `-` only in `<<-` and `>&-`.
            if self.peek() == Some('-') && self.pos == start {
                break;
            }
            self.pos += 1;
        }
        let op: String = self.chars[start..self.pos].iter().collect();
        match op.as_str() {
            "<<" | "<<-" => b.heredoc = Some(op == "<<-"),
            // `2>&1`, `>&-`: the target is attached.
            o if o.ends_with('&') && self.peek().is_some_and(|c| c.is_ascii_digit()) => {
                while self.peek().is_some_and(|c| c.is_ascii_digit()) {
                    self.pos += 1;
                }
            }
            o if o.ends_with("&-") => {}
            _ => b.skip_word = true,
        }
    }

    fn read_heredocs(&mut self, b: &mut Builder) {
        for pending in std::mem::take(&mut b.pending) {
            let mut body = String::new();
            while self.pos < self.chars.len() {
                let end = self.chars[self.pos..]
                    .iter()
                    .position(|&c| c == '\n')
                    .map_or(self.chars.len(), |i| self.pos + i);
                let line: String = self.chars[self.pos..end].iter().collect();
                self.pos = (end + 1).min(self.chars.len());
                let line = if pending.strip_tabs {
                    line.trim_start_matches('\t').to_string()
                } else {
                    line
                };
                if line == pending.delimiter {
                    break;
                }
                body.push_str(&line);
                body.push('\n');
            }
            match b.commands.get_mut(pending.owner) {
                Some(owner) => owner.heredocs.push(body),
                None => b.current.heredocs.push(body),
            }
        }
    }

    fn double_quoted(&mut self, b: &mut Builder) {
        b.word();
        while let Some(c) = self.peek() {
            self.pos += 1;
            match c {
                '"' => return,
                '\\' => match self.peek() {
                    Some(next @ ('"' | '\\' | '$' | '`')) => {
                        self.pos += 1;
                        b.word().push(next);
                    }
                    Some('\n') => self.pos += 1,
                    _ => b.word().push('\\'),
                },
                '$' if self.peek() == Some('(') => {
                    let text = self.substitution();
                    b.word().push_str(&text);
                }
                '`' => {
                    let text = self.backquoted();
                    b.word().push_str(&text);
                }
                c => b.word().push(c),
            }
        }
    }

    /// At the `(` of `$(`: parses the substitution's commands into `nested`
    /// and returns its source text, which stays part of the word.
    fn substitution(&mut self) -> String {
        let start = self.pos - 1;
        self.pos += 1;
        let commands = self.list(true);
        self.nested.extend(commands);
        self.chars[start..self.pos].iter().collect()
    }

    /// After an opening backquote: returns the text up to the closing one.
    fn backquoted(&mut self) -> String {
        let mut text = String::from('`');
        while let Some(c) = self.peek() {
            self.pos += 1;
            text.push(c);
            match c {
                '\\' => {
                    if let Some(next) = self.peek() {
                        self.pos += 1;
                        text.push(next);
                    }
                }
                '`' => break,
                _ => {}
            }
        }
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(script: &str) -> Vec<Vec<String>> {
        parse(script)
            .commands
            .into_iter()
            .map(|c| c.words)
            .collect()
    }

    #[test]
    fn splits_at_operators_and_removes_quotes() {
        assert_eq!(
            words("cd 'a b' && cargo test -- --nocapture | tail -5; echo \"done\""),
            [
                vec!["cd", "a b"],
                vec!["cargo", "test", "--", "--nocapture"],
                vec!["tail", "-5"],
                vec!["echo", "done"],
            ]
        );
        let ops: Vec<_> = parse("a && b || c | d & e; f")
            .commands
            .iter()
            .map(|c| c.op)
            .collect();
        assert_eq!(
            ops,
            [
                Some(Op::And),
                Some(Op::Or),
                Some(Op::Pipe),
                Some(Op::Background),
                Some(Op::Seq),
                None
            ]
        );
    }

    #[test]
    fn a_quoted_operator_is_part_of_the_word() {
        assert_eq!(
            words("git commit -m 'fix; git add -A && push'"),
            [vec!["git", "commit", "-m", "fix; git add -A && push"]]
        );
    }

    #[test]
    fn redirections_are_dropped() {
        assert_eq!(
            words("cargo test >out.txt 2>&1 < /dev/null &>> all.log"),
            [vec!["cargo", "test"]]
        );
    }

    #[test]
    fn reserved_words_and_assignments_are_stripped() {
        assert_eq!(
            words("while true; do RUST_LOG=1 cargo test; sleep 5; done"),
            [vec!["true"], vec!["cargo", "test"], vec!["sleep", "5"]]
        );
    }

    #[test]
    fn heredoc_bodies_are_kept_apart_from_words() {
        let script = parse("python3 - <<'EOF'\nprint('a; b')\nEOF\necho next");
        assert_eq!(script.commands[0].words, ["python3", "-"]);
        assert_eq!(script.commands[0].heredocs, ["print('a; b')\n"]);
        assert_eq!(script.commands[1].words, ["echo", "next"]);
    }

    #[test]
    fn a_heredoc_inside_a_quoted_substitution_may_contain_quotes() {
        let script = parse(
            "git commit -m \"$(cat <<'EOF'\nfix \"quoted\"; git add -A\nEOF\n)\" && git push",
        );
        assert_eq!(script.commands.len(), 2);
        assert_eq!(script.commands[0].words[..2], ["git", "commit"]);
        assert_eq!(script.commands[1].words, ["git", "push"]);
        assert_eq!(script.nested[0].words, ["cat"]);
        assert_eq!(script.nested[0].heredocs, ["fix \"quoted\"; git add -A\n"]);
    }

    #[test]
    fn wrappers_are_skipped_to_find_the_program() {
        let c = &parse("env -u GH_TOKEN FOO=1 timeout 60 rtk git status").commands[0];
        assert_eq!(c.argv(), ["git", "status"]);
        let c = &parse("mise x node@24 -- npm test").commands[0];
        assert_eq!(c.program(), Some("npm"));
        let c = &parse("/usr/bin/git.exe add").commands[0];
        assert_eq!(c.program(), Some("git"));
    }

    #[test]
    fn shell_c_scripts_are_parsed_too() {
        let script = parse("bash -lc 'sleep 30; gh pr checks'");
        let inner = script.inner_scripts();
        assert_eq!(inner[0].commands[0].words, ["sleep", "30"]);
        assert_eq!(inner[0].commands[1].words, ["gh", "pr", "checks"]);
    }

    fn status_of(script: &str, exit_code: i32) -> Vec<Option<i32>> {
        statuses(&parse(script).commands, exit_code)
    }

    #[test]
    fn a_failure_belongs_to_the_last_pipeline_only() {
        assert_eq!(status_of("cd crate && cargo test", 101), [None, Some(101)]);
        assert_eq!(status_of("cargo test; echo done", 1), [None, Some(1)]);
        assert_eq!(status_of("cargo test | tee log", 1), [Some(1), None]);
        assert_eq!(status_of("cargo test &", 0), [None]);
    }

    #[test]
    fn success_belongs_to_the_final_chain() {
        assert_eq!(
            status_of("cd x; cargo build && cargo test | tail -5", 0),
            [None, Some(0), Some(0), Some(0)]
        );
        assert_eq!(status_of("cargo test || true", 0), [None, Some(0)]);
    }

    #[test]
    fn line_continuations_and_comments() {
        assert_eq!(
            words("cargo \\\n  test # git add -A\n"),
            [vec!["cargo", "test"]]
        );
    }
}

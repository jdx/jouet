//! Which lines of which files a tool call reads, so the hooks can remember
//! what a session was shown and notice when it reads the same lines again.
//!
//! Understood: Claude Code's Read tool (`file_path`, `offset`, `limit`), and
//! shell calls made only of `cat`, `head`, `tail`, `sed -n 'A,Bp'` and `nl`,
//! alone or piped into one another, as in `nl -ba src/x.rs | sed -n
//! '10,40p'`. Anything else reads nothing jouet can account for.

use jouet_rules::Call;
use jouet_rules::shell;
use jouet_search::memory::{self, Memory, Stamp};
use std::path::{Path, PathBuf};

/// How many lines Claude Code's Read tool returns without a `limit`.
const READ_DEFAULT_LIMIT: u32 = 2000;
/// Files larger than this are not counted, so a hook never reads a huge
/// file to find its length.
const MAX_FILE: u64 = 8 * 1024 * 1024;

/// Lines of a file, 1-based and inclusive; `end` is `None` for "to the
/// end".
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Read {
    pub path: PathBuf,
    pub start: u32,
    pub end: Option<u32>,
    /// Counted from the end, as `tail -n N` reads.
    pub from_end: Option<u32>,
}

/// What the call reads, or `None` when it does anything else.
pub(crate) fn of(call: &Call) -> Option<Vec<Read>> {
    let cwd = call.cwd.as_deref().map(Path::new);
    let resolve = |p: &str| match cwd {
        Some(cwd) => cwd.join(p),
        None => PathBuf::from(p),
    };
    if call.tool == "Read" {
        let path = resolve(call.path.as_deref()?);
        let input = call.input.as_ref();
        let number = |key: &str| {
            input
                .and_then(|i| i.get(key))
                .and_then(serde_json::Value::as_u64)
                .and_then(|n| u32::try_from(n).ok())
        };
        let start = number("offset").unwrap_or(1).max(1);
        let limit = number("limit").unwrap_or(READ_DEFAULT_LIMIT).max(1);
        return Some(vec![Read {
            path,
            start,
            end: Some(start.saturating_add(limit - 1)),
            from_end: None,
        }]);
    }
    if call.tool != "Bash" {
        return None;
    }
    let command = call.command.as_deref()?;
    // The parser drops redirections, and `cat a > b` reads nothing into the
    // context.
    if command.contains('>') {
        return None;
    }
    let script = shell::parse(command);
    let commands = script.all();
    if commands.is_empty() || !script.inner_scripts().is_empty() {
        return None;
    }
    let mut reads: Vec<Read> = Vec::new();
    for command in commands {
        let argv = command.argv();
        let (program, args) = argv.split_first()?;
        if !command.heredocs.is_empty() {
            return None;
        }
        match shell::basename(program) {
            name @ ("cat" | "nl") => {
                let valued: &[&str] = if name == "nl" {
                    &["-b", "-d", "-f", "-h", "-i", "-l", "-n", "-s", "-v", "-w"]
                } else {
                    &[]
                };
                let files = files(args, valued)?;
                if files.is_empty() {
                    return None;
                }
                reads.extend(files.iter().map(|f| whole(resolve(f))));
            }
            "head" | "tail" => {
                let tail = shell::basename(program) == "tail";
                let (count, file) = head_tail(args)?;
                let path = match file {
                    Some(file) => resolve(file),
                    None => reads.pop()?.path,
                };
                reads.push(match (tail, count) {
                    (false, Count::Lines(n)) => Read {
                        path,
                        start: 1,
                        end: Some(n.max(1)),
                        from_end: None,
                    },
                    (true, Count::Lines(n)) => Read {
                        path,
                        start: 1,
                        end: None,
                        from_end: Some(n),
                    },
                    (true, Count::From(n)) => Read {
                        path,
                        start: n.max(1),
                        end: None,
                        from_end: None,
                    },
                    (false, Count::From(_)) => return None,
                });
            }
            "sed" => {
                let (start, end, file) = sed_print(args)?;
                let path = match file {
                    Some(file) => resolve(file),
                    None => reads.pop()?.path,
                };
                reads.push(Read {
                    path,
                    start,
                    end,
                    from_end: None,
                });
            }
            _ => return None,
        }
    }
    (!reads.is_empty()).then_some(reads)
}

fn whole(path: PathBuf) -> Read {
    Read {
        path,
        start: 1,
        end: None,
        from_end: None,
    }
}

/// The file arguments of a command, skipping its options and the values of
/// those in `valued`.
fn files<'a>(args: &'a [String], valued: &[&str]) -> Option<Vec<&'a String>> {
    let mut files = Vec::new();
    let mut skip = false;
    for arg in args {
        if std::mem::take(&mut skip) {
            continue;
        }
        if arg == "-" {
            return None;
        }
        if arg.starts_with('-') && arg.len() > 1 {
            skip = valued.contains(&arg.as_str());
            continue;
        }
        files.push(arg);
    }
    Some(files)
}

enum Count {
    Lines(u32),
    /// `tail -n +N`: from line N on.
    From(u32),
}

/// `head`/`tail` with `-n N`, `-nN`, `-N` or `-n +N`, and at most one file.
fn head_tail(args: &[String]) -> Option<(Count, Option<&String>)> {
    let mut count = Count::Lines(10);
    let mut file = None;
    let mut i = 0;
    let parse = |value: &str| -> Option<Count> {
        match value.strip_prefix('+') {
            Some(n) => n.parse().ok().map(Count::From),
            None => value.parse().ok().map(Count::Lines),
        }
    };
    while i < args.len() {
        let arg = &args[i];
        if arg == "-n" || arg == "--lines" {
            count = parse(args.get(i + 1)?)?;
            i += 2;
            continue;
        }
        if let Some(value) = arg
            .strip_prefix("--lines=")
            .or_else(|| arg.strip_prefix("-n"))
        {
            count = parse(value)?;
        } else if let Some(n) = arg.strip_prefix('-').filter(|n| !n.is_empty()) {
            count = Count::Lines(n.parse().ok()?);
        } else if file.replace(arg).is_some() {
            return None;
        }
        i += 1;
    }
    Some((count, file))
}

/// `sed -n 'A,Bp' [FILE]`, `'Ap'` or `'A,$p'`: the lines it prints.
fn sed_print(args: &[String]) -> Option<(u32, Option<u32>, Option<&String>)> {
    let mut quiet = false;
    let mut script = None;
    let mut file = None;
    for arg in args {
        if arg == "-n" || arg == "--quiet" || arg == "--silent" {
            quiet = true;
        } else if arg.starts_with('-') {
            return None;
        } else if script.is_none() {
            script = Some(arg.as_str());
        } else if file.replace(arg).is_some() {
            return None;
        }
    }
    let range = script?.strip_suffix('p')?;
    if !quiet {
        return None;
    }
    let (start, end) = match range.split_once(',') {
        Some((a, "$")) => (a.parse().ok()?, None),
        Some((a, b)) => (a.parse().ok()?, Some(b.parse().ok()?)),
        None => {
            let a = range.parse().ok()?;
            (a, Some(a))
        }
    };
    Some((start, end, file))
}

/// A read's lines in a file of `lines` lines, or `None` if it reads none.
fn span(read: &Read, lines: u32) -> Option<(u32, u32)> {
    if lines == 0 {
        return None;
    }
    let (start, end) = match read.from_end {
        Some(n) => (lines.saturating_sub(n).saturating_add(1).max(1), lines),
        None => (read.start, read.end.unwrap_or(lines).min(lines)),
    };
    (start <= end).then_some((start, end))
}

/// The file's stamp and line count, unless it is missing or too large.
fn measure(path: &Path) -> Option<(Stamp, u32)> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > MAX_FILE {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    let mut lines = bytes.iter().filter(|b| **b == b'\n').count();
    if bytes.last().is_some_and(|b| *b != b'\n') {
        lines += 1;
    }
    Some((Stamp::of(&meta), u32::try_from(lines).ok()?))
}

/// Whether the session was already shown every line the reads cover, in
/// files unchanged since.
pub(crate) fn seen(memory: &Memory, reads: &[Read]) -> bool {
    !reads.is_empty()
        && reads.iter().all(|read| {
            measure(&read.path).is_some_and(|(stamp, lines)| {
                span(read, lines).is_some_and(|(start, end)| {
                    memory.shown_range(&memory::key(&read.path), stamp, start, end)
                })
            })
        })
}

/// How many lines of files the reads put in the context, or `None` when a
/// file is missing or too large to count.
pub(crate) fn line_count(reads: &[Read]) -> Option<u32> {
    reads.iter().try_fold(0u32, |total, read| {
        let (_, lines) = measure(&read.path)?;
        let span = span(read, lines).map_or(0, |(start, end)| end - start + 1);
        Some(total.saturating_add(span))
    })
}

/// Remembers that the session was shown what the reads cover.
pub(crate) fn record(memory: &mut Memory, reads: &[Read]) {
    for read in reads {
        if let Some((stamp, lines)) = measure(&read.path)
            && let Some((start, end)) = span(read, lines)
        {
            memory.record(&memory::key(&read.path), stamp, start, end);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bash(command: &str) -> Call {
        Call {
            tool: "Bash".into(),
            command: Some(command.into()),
            ..Call::default()
        }
    }

    /// Each read as `PATH:START-END`, `$` for the end of the file, or
    /// `PATH:last N`.
    fn lines(command: &str) -> Option<Vec<String>> {
        of(&bash(command)).map(|reads| {
            reads
                .into_iter()
                .map(|r| match (r.from_end, r.end) {
                    (Some(n), _) => format!("{}:last {n}", r.path.display()),
                    (None, Some(end)) => format!("{}:{}-{end}", r.path.display(), r.start),
                    (None, None) => format!("{}:{}-$", r.path.display(), r.start),
                })
                .collect()
        })
    }

    #[test]
    fn shell_reads_are_understood() {
        let cases: &[(&str, &[&str])] = &[
            ("cat a.rs b.rs", &["a.rs:1-$", "b.rs:1-$"]),
            ("cat -n a.rs", &["a.rs:1-$"]),
            ("nl -ba a.rs", &["a.rs:1-$"]),
            ("nl -b a a.rs", &["a.rs:1-$"]),
            ("head -n 40 a.rs", &["a.rs:1-40"]),
            ("head -40 a.rs", &["a.rs:1-40"]),
            ("head a.rs", &["a.rs:1-10"]),
            ("tail -n 5 a.rs", &["a.rs:last 5"]),
            ("tail -n +20 a.rs", &["a.rs:20-$"]),
            ("sed -n '10,40p' a.rs", &["a.rs:10-40"]),
            ("sed -n 7p a.rs", &["a.rs:7-7"]),
            ("sed -n '10,$p' a.rs", &["a.rs:10-$"]),
            ("nl -ba a.rs | sed -n '10,40p'", &["a.rs:10-40"]),
            ("cat a.rs | head -n 3", &["a.rs:1-3"]),
        ];
        for (command, want) in cases {
            let want: Vec<String> = want.iter().map(|w| w.to_string()).collect();
            assert_eq!(lines(command), Some(want), "{command}");
        }
    }

    #[test]
    fn anything_else_reads_nothing_accountable() {
        for command in [
            "cat",
            "cat - < a.rs",
            "cat a.rs > b.rs",
            "cat a.rs 2>/dev/null",
            "sed 's/a/b/' a.rs",
            "sed -i -n '1p' a.rs",
            "head -c 10 a.rs",
            "cat a.rs && cargo test",
            "grep x a.rs",
            "cat <<EOF\nx\nEOF",
            "echo $(cat a.rs)",
            "tail -f log",
        ] {
            assert_eq!(lines(command), None, "{command}");
        }
    }

    #[test]
    fn a_read_is_seen_once_its_lines_were_shown_and_the_file_is_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.rs");
        std::fs::write(&file, "1\n2\n3\n4\n5\n").unwrap();
        let state = tempfile::tempdir().unwrap();
        let mut memory = Memory::open(state.path(), "s");
        let read = |start, end| Read {
            path: file.clone(),
            start,
            end,
            from_end: None,
        };
        assert!(!seen(&memory, &[read(1, None)]));
        record(&mut memory, &[read(2, Some(4))]);
        assert!(seen(&memory, &[read(3, Some(4))]));
        assert!(!seen(&memory, &[read(1, None)]));
        record(&mut memory, &[read(1, None)]);
        assert!(seen(&memory, &[read(1, None)]));
        let tail = Read {
            path: file.clone(),
            start: 1,
            end: None,
            from_end: Some(2),
        };
        assert!(seen(&memory, &[tail]));
        // A Read past the end covers only the lines that exist.
        assert!(seen(&memory, &[read(1, Some(2000))]));
        assert_eq!(line_count(&[read(2, None)]), Some(4));
        assert_eq!(
            line_count(&[read(1, Some(2000)), read(4, Some(4))]),
            Some(6)
        );
        std::fs::write(&file, "1\n2\n3\n4\n5\n6\n").unwrap();
        assert!(!seen(&memory, &[read(1, Some(2))]));
    }

    #[test]
    fn the_read_tool_reads_its_offset_and_limit() {
        let call = Call {
            tool: "Read".into(),
            path: Some("a.rs".into()),
            cwd: Some("/repo".into()),
            input: Some(serde_json::json!({"file_path": "a.rs", "offset": 10, "limit": 5})),
            ..Call::default()
        };
        let read = of(&call).unwrap().remove(0);
        assert_eq!(
            (read.path, read.start, read.end),
            (PathBuf::from("/repo/a.rs"), 10, Some(14))
        );
    }
}

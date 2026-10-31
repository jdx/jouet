//! The lines a patch adds: `git diff -U0` or `git log -p -U0`, read into
//! one piece per hunk with the line its added text starts at.

/// Where the commits in a `git log -p` start: `--format=` this, then the
/// full SHA.
pub(crate) const COMMIT_MARK: &str = "%x00%H";

/// The lines one hunk adds to one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Piece {
    pub commit: Option<String>,
    pub path: String,
    /// The line the added text starts at, in the new file.
    pub first_line: u64,
    pub text: Vec<u8>,
}

/// What a patch adds: its hunks, and every file it adds or changes (a
/// binary file among them), with the commit that did.
#[derive(Debug, Default)]
pub(crate) struct Patch {
    pub pieces: Vec<Piece>,
    pub files: Vec<(Option<String>, String)>,
    pub commits: usize,
}

/// Reads a patch with `a/` and `b/` prefixes. A log's commits each start
/// with a NUL and the SHA, from `--format=%x00%H`.
pub(crate) fn parse(patch: &[u8]) -> Patch {
    let mut out = Patch::default();
    let mut commit: Option<String> = None;
    let mut path: Option<String> = None;
    // The old and new lines left in the current hunk.
    let mut left = (0u64, 0u64);
    let mut piece: Option<Piece> = None;
    for line in patch.split(|b| *b == b'\n') {
        if left != (0, 0) {
            let in_hunk = match line.first() {
                Some(b'+') => {
                    left.1 = left.1.saturating_sub(1);
                    if let Some(p) = piece.as_mut() {
                        p.text.extend_from_slice(&line[1..]);
                        p.text.push(b'\n');
                    }
                    true
                }
                Some(b'-') => {
                    left.0 = left.0.saturating_sub(1);
                    true
                }
                Some(b' ') => {
                    left.0 = left.0.saturating_sub(1);
                    left.1 = left.1.saturating_sub(1);
                    true
                }
                // `\ No newline at end of file`.
                Some(b'\\') => true,
                // A hunk cut short: read the line as a header.
                _ => {
                    left = (0, 0);
                    false
                }
            };
            if left == (0, 0) {
                flush(&mut piece, &mut out);
            }
            if in_hunk {
                continue;
            }
        }
        if let Some(sha) = line.strip_prefix(b"\0") {
            commit = Some(String::from_utf8_lossy(sha).trim().to_string());
            out.commits += 1;
            path = None;
        } else if line.starts_with(b"diff --git ") {
            path = None;
        } else if let Some(name) = line.strip_prefix(b"+++ ") {
            path = file_name(name);
            if let Some(p) = &path {
                out.files.push((commit.clone(), p.clone()));
            }
        } else if let Some(rest) = line.strip_prefix(b"Binary files ") {
            // `Binary files a/x and b/x differ`.
            let rest = rest.strip_suffix(b" differ").unwrap_or(rest);
            let new = rest
                .windows(5)
                .rposition(|w| w == b" and ")
                .map(|at| &rest[at + 5..]);
            if let Some(p) = new.and_then(file_name) {
                out.files.push((commit.clone(), p));
            }
        } else if line.starts_with(b"@@ ") {
            let Some((old, new, start)) = hunk_header(line) else {
                continue;
            };
            left = (old, new);
            piece = path.clone().map(|path| Piece {
                commit: commit.clone(),
                path,
                first_line: start,
                text: Vec::new(),
            });
            if left == (0, 0) {
                piece = None;
            }
        }
    }
    flush(&mut piece, &mut out);
    out
}

fn flush(piece: &mut Option<Piece>, out: &mut Patch) {
    if let Some(p) = piece.take().filter(|p| !p.text.is_empty()) {
        out.pieces.push(p);
    }
}

/// `@@ -a,b +c,d @@`: the old and new line counts, and where the new lines
/// start.
fn hunk_header(line: &[u8]) -> Option<(u64, u64, u64)> {
    let text = std::str::from_utf8(line).ok()?;
    let mut words = text.split(' ').skip(1);
    let old = words.next()?.strip_prefix('-')?;
    let new = words.next()?.strip_prefix('+')?;
    let count = |range: &str| -> Option<(u64, u64)> {
        match range.split_once(',') {
            Some((start, n)) => Some((start.parse().ok()?, n.parse().ok()?)),
            None => Some((range.parse().ok()?, 1)),
        }
    };
    let (_, old_n) = count(old)?;
    let (start, new_n) = count(new)?;
    Some((old_n, new_n, start))
}

/// A `b/` name from a `+++` or `Binary files` line; `None` for
/// `/dev/null`. A name git quoted is unquoted.
fn file_name(name: &[u8]) -> Option<String> {
    let name = name.strip_suffix(b"\t").unwrap_or(name);
    let name = match name.strip_prefix(b"\"").and_then(|n| n.strip_suffix(b"\"")) {
        Some(quoted) => unquote(quoted),
        None => name.to_vec(),
    };
    let rest = name.strip_prefix(b"b/")?;
    Some(String::from_utf8_lossy(rest).into_owned())
}

/// Undoes git's C-style quoting of a path.
fn unquote(quoted: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(quoted.len());
    let mut i = 0;
    while i < quoted.len() {
        let b = quoted[i];
        if b != b'\\' || i + 1 == quoted.len() {
            out.push(b);
            i += 1;
            continue;
        }
        let next = quoted[i + 1];
        let octal = |at: usize| quoted.get(at).filter(|c| (b'0'..=b'7').contains(c));
        match next {
            b'n' => out.push(b'\n'),
            b't' => out.push(b'\t'),
            b'r' => out.push(b'\r'),
            b'a' => out.push(7),
            b'b' => out.push(8),
            b'f' => out.push(12),
            b'v' => out.push(11),
            b'0'..=b'7' if octal(i + 2).is_some() && octal(i + 3).is_some() => {
                let value = (u32::from(next - b'0') << 6)
                    | (u32::from(quoted[i + 2] - b'0') << 3)
                    | u32::from(quoted[i + 3] - b'0');
                out.push(u8::try_from(value).unwrap_or(b'?'));
                i += 4;
                continue;
            }
            other => out.push(other),
        }
        i += 2;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn added_lines_with_where_they_start() {
        let patch = b"diff --git a/src/a.rs b/src/a.rs
index 1..2 100644
--- a/src/a.rs
+++ b/src/a.rs
@@ -3 +3,2 @@ fn x() {
-old
+new one
++++ starts with pluses
@@ -10,0 +12 @@
+later
diff --git a/gone.txt b/gone.txt
deleted file mode 100644
--- a/gone.txt
+++ /dev/null
@@ -1 +0,0 @@
-bye
diff --git a/key.p12 b/key.p12
new file mode 100644
Binary files /dev/null and b/key.p12 differ
diff --git \"a/sp ace\\303\\251.txt\" \"b/sp ace\\303\\251.txt\"
--- /dev/null
+++ \"b/sp ace\\303\\251.txt\"
@@ -0,0 +1 @@
+x
\\ No newline at end of file
";
        let p = parse(patch);
        let got: Vec<(&str, u64, &str)> = p
            .pieces
            .iter()
            .map(|p| {
                (
                    p.path.as_str(),
                    p.first_line,
                    std::str::from_utf8(&p.text).unwrap(),
                )
            })
            .collect();
        assert_eq!(
            got,
            [
                ("src/a.rs", 3, "new one\n+++ starts with pluses\n"),
                ("src/a.rs", 12, "later\n"),
                ("sp aceé.txt", 1, "x\n"),
            ]
        );
        let files: Vec<&str> = p.files.iter().map(|(_, f)| f.as_str()).collect();
        assert_eq!(files, ["src/a.rs", "key.p12", "sp aceé.txt"]);
        assert_eq!(p.commits, 0);
    }

    #[test]
    fn a_log_names_the_commit_of_each_piece() {
        let patch = b"\x00aaaa
diff --git a/x b/x
--- a/x
+++ b/x
@@ -1 +1 @@
-a
+b
\x00bbbb

diff --git a/x b/x
--- a/x
+++ b/x
@@ -1 +1 @@
-b
+c
";
        let p = parse(patch);
        let commits: Vec<Option<&str>> = p.pieces.iter().map(|p| p.commit.as_deref()).collect();
        assert_eq!(commits, [Some("aaaa"), Some("bbbb")]);
        assert_eq!(p.commits, 2);
    }
}

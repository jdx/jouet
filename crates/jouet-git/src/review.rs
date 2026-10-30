//! `jouet review-package BASE HEAD`: one file holding everything a reviewer
//! needs for a range of commits: the commits, a diffstat, and the full diff
//! with extra context.
//!
//! The range must be non-empty and BASE an ancestor of HEAD; otherwise the
//! diff would show changes the reviewer is not being asked about (or
//! nothing), so it is refused before anything is written.

use crate::{Error, Git, nul_fields};
use jouet_output::exit::Exit;
use serde::Serialize;
use std::fmt::Write as _;

/// Lines of context around each change unless asked otherwise.
pub const DEFAULT_CONTEXT: u32 = 10;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Rev {
    /// As given on the command line.
    pub name: String,
    pub sha: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Package {
    pub base: Rev,
    pub head: Rev,
    pub commits: u32,
    pub files: u32,
    pub insertions: u64,
    pub deletions: u64,
    /// The package itself, as markdown.
    #[serde(skip)]
    pub text: String,
}

/// Builds the package for `base..head` with `context` lines around each
/// change.
pub fn build(git: &Git, base: &str, head: &str, context: u32) -> Result<Package, Error> {
    let resolve = |name: &str| {
        git.commit_sha(name)
            .map(|sha| Rev {
                name: name.to_string(),
                sha,
            })
            .ok_or_else(|| Error::new(Exit::NotFound, format!("`{name}` names no commit here")))
    };
    let (base, head) = (resolve(base)?, resolve(head)?);
    if !git.is_ancestor(&base.sha, &head.sha) {
        let hint = if git.is_ancestor(&head.sha, &base.sha) {
            format!(
                "the two look swapped: jouet review-package {} {}",
                head.name, base.name
            )
        } else {
            match git.line(&["merge-base", &base.sha, &head.sha]) {
                Some(mb) => format!(
                    "their merge base is {}; to review what {} adds since then: jouet review-package {} {}",
                    short(&mb),
                    head.name,
                    short(&mb),
                    head.name
                ),
                None => "the two commits share no history".into(),
            }
        };
        return Err(Error::new(
            Exit::Precondition,
            format!(
                "{} is not an ancestor of {}, so the diff would include changes {} does not make",
                base.name, head.name, head.name
            ),
        )
        .hint(hint));
    }
    let range = format!("{}..{}", base.sha, head.sha);
    let commits = git.count(&range).unwrap_or(0);
    if commits == 0 {
        return Err(Error::new(
            Exit::Precondition,
            format!(
                "{}..{} is empty: there is nothing to review",
                base.name, head.name
            ),
        ));
    }

    let log = git.text(&[
        "log",
        "--reverse",
        "--no-color",
        "--date=short",
        "--format=- %h %s (%an, %ad)",
        &range,
    ])?;
    let numstat = git.text(&[
        "diff",
        "--no-color",
        "--find-renames",
        "--numstat",
        "-z",
        &base.sha,
        &head.sha,
    ])?;
    let (files, insertions, deletions) = totals(&numstat);
    let stat = git.text(&[
        "diff",
        "--no-color",
        "--find-renames",
        "--stat=120",
        "--summary",
        &base.sha,
        &head.sha,
    ])?;
    let diff = git.text(&[
        "diff",
        "--no-color",
        "--no-ext-diff",
        "--find-renames",
        &format!("-U{context}"),
        &base.sha,
        &head.sha,
    ])?;

    let mut text = String::new();
    let _ = writeln!(text, "# Review: {}..{}\n", base.name, head.name);
    let _ = writeln!(text, "- base: {} ({})", base.sha, base.name);
    let _ = writeln!(text, "- head: {} ({})", head.sha, head.name);
    let _ = writeln!(
        text,
        "- {commits} {}, {files} {} changed, +{insertions} -{deletions}",
        plural(u64::from(commits), "commit", "commits"),
        plural(u64::from(files), "file", "files")
    );
    let _ = writeln!(text, "- diff context: {context} lines\n");
    let _ = writeln!(text, "## Commits\n\n{}", log.trim_end());
    let _ = writeln!(text, "\n## Diffstat\n");
    fenced(&mut text, "", &stat);
    let _ = writeln!(text, "\n## Diff\n");
    fenced(&mut text, "diff", &diff);
    Ok(Package {
        base,
        head,
        commits,
        files,
        insertions,
        deletions,
        text,
    })
}

fn short(sha: &str) -> &str {
    sha.get(..12).unwrap_or(sha)
}

fn plural<'a>(n: u64, one: &'a str, many: &'a str) -> &'a str {
    if n == 1 { one } else { many }
}

/// Files, insertions and deletions from `--numstat -z`. A rename is one
/// entry whose path fields follow it.
fn totals(numstat: &str) -> (u32, u64, u64) {
    let (mut files, mut ins, mut del) = (0, 0, 0);
    let mut fields = nul_fields(numstat);
    while let Some(field) = fields.next() {
        let mut parts = field.splitn(3, '\t');
        let (a, d, path) = (parts.next(), parts.next(), parts.next());
        files += 1;
        ins += a.and_then(|n| n.parse::<u64>().ok()).unwrap_or(0);
        del += d.and_then(|n| n.parse::<u64>().ok()).unwrap_or(0);
        if path == Some("") {
            // A rename: the old and new paths are the next two fields.
            fields.next();
            fields.next();
        }
    }
    (files, ins, del)
}

/// Appends `body` in a code fence longer than any run of backticks inside
/// it, so a diff of a markdown file cannot close the fence early.
fn fenced(out: &mut String, lang: &str, body: &str) {
    let longest = body.split(|c| c != '`').map(str::len).max().unwrap_or(0);
    let fence = "`".repeat(longest.max(2) + 1);
    let _ = writeln!(out, "{fence}{lang}");
    out.push_str(body);
    if !body.ends_with('\n') {
        out.push('\n');
    }
    let _ = writeln!(out, "{fence}");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Repo;

    #[test]
    fn packages_a_range() {
        let repo = Repo::new();
        let lines: String = (1..=20).map(|i| format!("line {i}\n")).collect();
        let base = repo.commit(&[("a.txt", &lines)], "init");
        let changed = lines.replace("line 3\n", "line three\n");
        repo.commit(&[("a.txt", &changed)], "change a");
        repo.git(&["mv", "a.txt", "b.txt"]);
        repo.commit(&[("doc.md", "```sh\nls\n```\n")], "move a, add doc");

        let pkg = build(&repo.git_handle(), &base, "HEAD", 3).unwrap();

        assert_eq!(pkg.commits, 2);
        assert_eq!(pkg.files, 2);
        assert_eq!((pkg.insertions, pkg.deletions), (4, 1));
        assert_eq!(pkg.head.name, "HEAD");
        let text = &pkg.text;
        assert!(
            text.contains("- 2 commits, 2 files changed, +4 -1"),
            "{text}"
        );
        let change = text.find(" change a (").expect(text);
        let moved = text.find(" move a, add doc (").unwrap();
        assert!(change < moved, "oldest first: {text}");
        assert!(text.contains("## Diffstat\n\n```\n"), "{text}");
        assert!(text.contains("rename a.txt => b.txt ("), "{text}");
        // The doc's own fence is inside a longer one.
        assert!(text.contains("````diff\n"), "{text}");
        assert!(text.contains("+```sh\n"), "{text}");
        assert!(text.trim_end().ends_with("````"), "{text}");
    }

    #[test]
    fn refuses_an_empty_range_or_a_base_off_the_branch() {
        let repo = Repo::new();
        let git = repo.git_handle();
        let root = repo.commit(&[("a.txt", "a\n")], "init");
        repo.git(&["checkout", "-q", "-b", "side"]);
        let side = repo.commit(&[("side.txt", "s\n")], "side");
        repo.git(&["checkout", "-q", "main"]);
        repo.commit(&[("main.txt", "m\n")], "main");

        let err = build(&git, "HEAD", "HEAD", 3).unwrap_err();
        assert_eq!(err.exit, Exit::Precondition);
        assert!(err.message.contains("is empty"), "{err:?}");

        let err = build(&git, "side", "main", 3).unwrap_err();
        assert_eq!(err.exit, Exit::Precondition);
        assert!(err.message.contains("not an ancestor"), "{err:?}");
        let hint = err.hint.unwrap();
        assert!(hint.contains(short(&root)), "{hint}");

        let err = build(&git, "main", &root, 3).unwrap_err();
        assert!(err.hint.unwrap().contains("swapped"));

        let err = build(&git, "nope", "main", 3).unwrap_err();
        assert_eq!(err.exit, Exit::NotFound);

        assert_eq!(build(&git, &root, &side, 3).unwrap().commits, 1);
    }

    #[test]
    fn fences_outgrow_the_backticks_inside() {
        let mut out = String::new();
        fenced(&mut out, "", "plain");
        assert_eq!(out, "```\nplain\n```\n");
        let mut out = String::new();
        fenced(&mut out, "diff", "+`````\n");
        assert_eq!(out, "``````diff\n+`````\n``````\n");
    }
}

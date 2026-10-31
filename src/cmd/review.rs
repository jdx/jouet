//! `jouet review-package`.

use super::Output;
use crate::cli::ReviewPackageArgs;
use jouet_git::Git;
use jouet_git::review::{self, DEFAULT_CONTEXT, Package};
use jouet_output::exit::Exit;
use serde::Serialize;
use std::path::PathBuf;

#[derive(Serialize)]
struct Report<'a> {
    /// Absent when the package went to stdout.
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<String>,
    #[serde(flatten)]
    package: &'a Package,
    /// The package itself, in JSON output when it went to stdout.
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<&'a str>,
}

pub(crate) fn run(args: &ReviewPackageArgs, out: &Output) -> Exit {
    let git = Git::here();
    let head = args.head.as_deref().unwrap_or("HEAD");
    let context = args.context.unwrap_or(DEFAULT_CONTEXT);
    let package = match review::build(&git, &args.base, head, context) {
        Ok(package) => package,
        Err(e) => return out.error_with_hint(e.exit, &e.message, e.hint.as_deref()),
    };
    if args.output.as_deref() == Some("-") {
        let report = Report {
            path: None,
            package: &package,
            text: Some(&package.text),
        };
        out.result(&report, || package.text.clone());
        return Exit::Ok;
    }
    let path = match &args.output {
        Some(path) => PathBuf::from(path),
        None => match git.git_dir() {
            Ok(dir) => dir.join("jouet").join(format!(
                "review-{}-{}.md",
                short(&package.base.sha),
                short(&package.head.sha)
            )),
            Err(e) => return out.error(e.exit, &e.message),
        },
    };
    if let Err(e) = write(&path, &package.text) {
        return out.error(Exit::Failed, &format!("writing {}: {e}", path.display()));
    }
    let path = std::path::absolute(&path).unwrap_or(path);
    let report = Report {
        path: Some(path.display().to_string()),
        package: &package,
        text: None,
    };
    out.result(&report, || {
        format!(
            "wrote {}\n  {}..{}: {} {}, {} {} changed, +{} -{}\n",
            path.display(),
            package.base.name,
            package.head.name,
            package.commits,
            if package.commits == 1 {
                "commit"
            } else {
                "commits"
            },
            package.files,
            if package.files == 1 { "file" } else { "files" },
            package.insertions,
            package.deletions
        )
    });
    Exit::Ok
}

fn short(sha: &str) -> &str {
    sha.get(..12).unwrap_or(sha)
}

fn write(path: &std::path::Path, text: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, text)
}

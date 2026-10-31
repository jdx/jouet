//! `jouet read --map`: Aider's repository map, in Rust.

use super::Output;
use crate::ReadArgs;
use jouet_output::exit::Exit;
use jouet_search::map::{self, DEFAULT_BUDGET, UNFOCUSED_MULTIPLIER};
use jouet_search::symbols;
use std::path::{Path, PathBuf};

pub(crate) fn run(args: &ReadArgs, out: &Output) -> Exit {
    if !args.map {
        return out.error(Exit::Usage, "give --map to print a map of the repository");
    }
    if symbols::languages().is_empty() {
        return out.error_with_hint(
            Exit::Usage,
            "this jouet was built without grammars, so it cannot read definitions",
            Some("install a release build, or build with `--features grammars`"),
        );
    }
    let root = PathBuf::from(args.path.as_deref().unwrap_or("."));
    if !root.is_dir() {
        return out.error(
            Exit::NotFound,
            &format!("{}: no such directory", root.display()),
        );
    }
    let (focus_files, focus_idents) = split_focus(&root, &args.focus);
    let budget = args
        .budget
        .unwrap_or(if focus_files.is_empty() && focus_idents.is_empty() {
            DEFAULT_BUDGET * UNFOCUSED_MULTIPLIER
        } else {
            DEFAULT_BUDGET
        });
    let options = map::Options {
        root,
        focus_files,
        focus_idents,
        budget,
        cache: jouet_output::paths::cache_dir().map(|d| d.join("tags")),
    };
    let repo_map = map::build(&options);
    out.result(&repo_map, || map::render(&repo_map));
    Exit::Ok
}

/// Splits `--focus` values into files, relative to the map's root, and
/// identifiers: a value naming an existing file is a file.
fn split_focus(root: &Path, focus: &[String]) -> (Vec<String>, Vec<String>) {
    let canonical_root = std::fs::canonicalize(root).ok();
    let mut files = Vec::new();
    let mut idents = Vec::new();
    for value in focus {
        let path = Path::new(value);
        let file = [path.to_path_buf(), root.join(path)]
            .into_iter()
            .find(|p| p.is_file())
            .and_then(|p| std::fs::canonicalize(p).ok());
        let rel = file.zip(canonical_root.as_ref()).and_then(|(file, root)| {
            file.strip_prefix(root).ok().map(|rel| {
                rel.components()
                    .map(|c| c.as_os_str().to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("/")
            })
        });
        match rel {
            Some(rel) => files.push(rel),
            None => idents.push(value.clone()),
        }
    }
    (files, idents)
}

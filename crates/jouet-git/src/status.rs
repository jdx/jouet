//! The state of a work tree from `git status --porcelain=v2 --branch`, as
//! the rules' `jouet.git.state()` probe and the routines' `git.state()`
//! report it.

use serde_json::{Value, json};

/// The branch, its upstream and how far apart they are, and whether the
/// work tree is clean.
pub fn parse(out: &str) -> Value {
    let (mut head, mut branch, mut upstream) = (None, None, None);
    let (mut ahead, mut behind): (Option<u64>, Option<u64>) = (None, None);
    let (mut changed, mut untracked, mut conflicted) = (0u64, 0u64, 0u64);
    for line in out.lines() {
        if let Some(header) = line.strip_prefix("# ") {
            let (key, value) = header.split_once(' ').unwrap_or((header, ""));
            match key {
                "branch.oid" if value != "(initial)" => {
                    head = Some(value.chars().take(12).collect::<String>());
                }
                "branch.head" if value != "(detached)" => branch = Some(value.to_string()),
                "branch.upstream" => upstream = Some(value.to_string()),
                "branch.ab" => {
                    let mut parts = value.split(' ');
                    ahead = parts
                        .next()
                        .and_then(|a| a.trim_start_matches('+').parse().ok());
                    behind = parts
                        .next()
                        .and_then(|b| b.trim_start_matches('-').parse().ok());
                }
                _ => {}
            }
        } else if line.starts_with("? ") {
            untracked += 1;
        } else if line.starts_with("u ") {
            conflicted += 1;
        } else if line.starts_with("1 ") || line.starts_with("2 ") {
            changed += 1;
        }
    }
    let mut state = json!({
        "detached": branch.is_none(),
        "dirty": changed + untracked + conflicted > 0,
        "changed": changed,
        "untracked": untracked,
        "conflicted": conflicted,
    });
    for (key, value) in [("head", head), ("branch", branch), ("upstream", upstream)] {
        if let Some(value) = value {
            state[key] = value.into();
        }
    }
    for (key, value) in [("ahead", ahead), ("behind", behind)] {
        if let Some(value) = value {
            state[key] = value.into();
        }
    }
    state
}

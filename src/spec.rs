//! Questions about jouet's own command tree, answered from the usage-rs
//! declaration.

use crate::Jouet;

/// Whether this binary has the command at `path`, such as `["wait", "ci"]`.
pub(crate) fn has_command(path: &[&str]) -> bool {
    let mut meta = Jouet::spec().root;
    for name in path {
        let found = meta
            .subcommands
            .iter()
            .find(|c| c.cmd.name == *name || c.cmd.aliases.contains(name));
        match found {
            Some(sub) => meta = sub,
            None => return false,
        }
    }
    true
}

/// What this binary lacks of a `jouet` command line, such as a memory
/// names: its first word that is not a subcommand where one is expected,
/// or a long flag no command on its path has. `words` follow `jouet`.
pub(crate) fn unknown_in(words: &[&str]) -> Option<String> {
    let root = Jouet::spec().root;
    let mut path = vec![root];
    let mut names: Vec<&str> = Vec::new();
    let mut args = false;
    for word in words {
        if let Some(long) = word.strip_prefix("--") {
            let long = long.split('=').next().unwrap_or_default();
            if long.is_empty() || matches!(long, "help" | "version") {
                continue;
            }
            let known = path
                .iter()
                .any(|m| m.flags.iter().any(|f| f.flag.longs.contains(&long)));
            if !known {
                return Some(format!("jouet {} --{long}", names.join(" ")).replace("  ", " "));
            }
            continue;
        }
        if word.starts_with('-') || args {
            continue;
        }
        let meta = path.last().expect("starts at the root");
        if meta.subcommands.is_empty() {
            args = true;
            continue;
        }
        match meta
            .subcommands
            .iter()
            .find(|c| c.cmd.name == *word || c.cmd.aliases.contains(word))
        {
            Some(sub) => {
                path.push(*sub);
                names.push(word);
            }
            // A command that takes arguments as well as subcommands.
            None if !meta.args.is_empty() => args = true,
            None => return Some(format!("jouet {} {word}", names.join(" ")).replace("  ", " ")),
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn walks_the_command_tree() {
        assert!(has_command(&["edit", "replace"]));
        assert!(!has_command(&["edit", "nope"]));
        assert!(!has_command(&["nope"]));
    }

    #[test]
    fn names_what_a_command_line_lacks() {
        assert_eq!(unknown_in(&["wind", "show", "n1"]), None);
        assert_eq!(unknown_in(&["pr", "threads", "--json"]), None);
        assert_eq!(unknown_in(&["wind", "--help"]), None);
        assert_eq!(
            unknown_in(&["frobnicate"]).as_deref(),
            Some("jouet frobnicate")
        );
        assert_eq!(
            unknown_in(&["wind", "nope"]).as_deref(),
            Some("jouet wind nope")
        );
        assert_eq!(
            unknown_in(&["wind", "review", "--no-such-flag"]).as_deref(),
            Some("jouet wind review --no-such-flag")
        );
    }
}

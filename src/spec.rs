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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn walks_the_command_tree() {
        assert!(has_command(&["edit", "replace"]));
        assert!(!has_command(&["edit", "nope"]));
        assert!(!has_command(&["nope"]));
    }
}

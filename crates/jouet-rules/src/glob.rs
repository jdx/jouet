//! The wildcards rule matchers accept: `*` for any run of characters and `?`
//! for one, in command words, arguments and tool names; and in paths, `*`
//! within one directory and `**` across any number of them.

/// Whether `text` matches `pattern`, where `*` matches any run of
/// characters, `/` included, and `?` any one character.
pub(crate) fn matches(pattern: &str, text: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let text: Vec<char> = text.chars().collect();
    wild(&pattern, &text, false)
}

/// Whether a file path matches `pattern`, as in `.gitignore`: a pattern
/// without a `/` matches the file's name, and one with a `/` matches the
/// whole path. `*` stays within a directory, and `**` matches any number of
/// them.
pub(crate) fn matches_path(pattern: &str, path: &str) -> bool {
    let path = path.replace('\\', "/");
    let pattern = pattern.strip_prefix("./").unwrap_or(pattern);
    if !pattern.contains('/') {
        let name = path.rsplit('/').next().unwrap_or(&path);
        return wild(
            &pattern.chars().collect::<Vec<_>>(),
            &name.chars().collect::<Vec<_>>(),
            true,
        );
    }
    let pattern: Vec<char> = pattern.chars().collect();
    let text: Vec<char> = path.chars().collect();
    wild(&pattern, &text, true)
}

/// Backtracking wildcard match. With `paths`, a single `*` or `?` never
/// crosses a `/`, and `**` does.
fn wild(p: &[char], t: &[char], paths: bool) -> bool {
    match p.first() {
        None => t.is_empty(),
        Some('*') if paths && p.get(1) == Some(&'*') => {
            // `**/` also matches no directory at all.
            let rest = &p[2..];
            let rest_after_slash = rest.strip_prefix(&['/']).unwrap_or(rest);
            (0..=t.len()).any(|i| wild(rest, &t[i..], paths))
                || (rest.len() != rest_after_slash.len() && wild(rest_after_slash, t, paths))
        }
        Some('*') => {
            let rest = &p[1..];
            for i in 0..=t.len() {
                if wild(rest, &t[i..], paths) {
                    return true;
                }
                if paths && t.get(i) == Some(&'/') {
                    return false;
                }
            }
            false
        }
        Some('?') => match t.first() {
            Some('/') if paths => false,
            Some(_) => wild(&p[1..], &t[1..], paths),
            None => false,
        },
        Some(c) => t.first() == Some(c) && wild(&p[1..], &t[1..], paths),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words() {
        assert!(matches("git", "git"));
        assert!(!matches("git", "gitk"));
        assert!(matches("python3.*", "python3.12"));
        assert!(!matches("python3.*", "python3"));
        assert!(matches("+?*", "+main"));
        assert!(!matches("+?*", "+"));
        assert!(matches(
            "*api.github.com/graphql*",
            "https://api.github.com/graphql"
        ));
        assert!(matches("*", ""));
    }

    #[test]
    fn paths() {
        assert!(matches_path("Cargo.lock", "/repo/Cargo.lock"));
        assert!(matches_path("*.lock", "/repo/sub/aube-lock.lock"));
        assert!(!matches_path("*.lock", "/repo/Cargo.lockfile"));
        assert!(matches_path("vendor/**", "vendor/a/b.rs"));
        assert!(matches_path("**/vendor/**", "/repo/vendor/a/b.rs"));
        assert!(matches_path("src/*.rs", "src/main.rs"));
        assert!(!matches_path("src/*.rs", "src/cmd/main.rs"));
        assert!(matches_path("src/**/*.rs", "src/main.rs"));
        assert!(matches_path("src/**/*.rs", "src/cmd/main.rs"));
        assert!(matches_path("./src/*.rs", "src/main.rs"));
        assert!(matches_path("*.rs", "C:\\repo\\main.rs"));
    }
}

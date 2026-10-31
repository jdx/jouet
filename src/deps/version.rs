//! Comparing versions well enough to find the first fixed one: semver
//! (crates.io, npm), Go's `v`-prefixed semver, and PEP 440's common forms.

use std::cmp::Ordering;

/// The release numbers, and what follows them: `1.2.3-rc.1` is `[1, 2,
/// 3]` and `rc.1`; `2.0.post1` is `[2, 0]` and `post1`.
fn parts(version: &str) -> (Vec<u64>, &str) {
    let v = version.trim().trim_start_matches(['v', 'V']);
    let v = v.split('+').next().unwrap_or(v);
    let end = v
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(v.len());
    let release = v[..end]
        .split('.')
        .filter(|p| !p.is_empty())
        .map(|p| p.parse().unwrap_or(u64::MAX))
        .collect();
    let rest = v[end..].trim_start_matches(['-', '.', '_']);
    (release, rest)
}

/// A suffix's rank against none: a post-release is newer, anything else
/// (a pre-release, a dev release) older.
fn suffix_rank(suffix: &str) -> i8 {
    if suffix.is_empty() {
        0
    } else if suffix.starts_with("post") {
        1
    } else {
        -1
    }
}

/// Words and numbers in turn, numbers compared as numbers: `rc.10` after
/// `rc.9`.
fn suffix_cmp(a: &str, b: &str) -> Ordering {
    let pieces = |s: &str| -> Vec<(bool, String)> {
        let mut out: Vec<(bool, String)> = Vec::new();
        for c in s.chars().filter(|c| c.is_ascii_alphanumeric()) {
            let digit = c.is_ascii_digit();
            match out.last_mut() {
                Some((d, piece)) if *d == digit => piece.push(c),
                _ => out.push((digit, c.to_string())),
            }
        }
        out
    };
    let (a, b) = (pieces(a), pieces(b));
    for (x, y) in a.iter().zip(&b) {
        let ord = match (x.0, y.0) {
            (true, true) => {
                x.1.parse::<u64>()
                    .unwrap_or(u64::MAX)
                    .cmp(&y.1.parse().unwrap_or(u64::MAX))
            }
            _ => x.1.cmp(&y.1),
        };
        if ord != Ordering::Equal {
            return ord;
        }
    }
    a.len().cmp(&b.len())
}

pub(crate) fn compare(a: &str, b: &str) -> Ordering {
    let (ra, sa) = parts(a);
    let (rb, sb) = parts(b);
    let len = ra.len().max(rb.len());
    let at = |r: &[u64], i: usize| r.get(i).copied().unwrap_or(0);
    for i in 0..len {
        let ord = at(&ra, i).cmp(&at(&rb, i));
        if ord != Ordering::Equal {
            return ord;
        }
    }
    suffix_rank(sa)
        .cmp(&suffix_rank(sb))
        .then_with(|| suffix_cmp(sa, sb))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_by_release_then_suffix() {
        let less = |a: &str, b: &str| {
            assert_eq!(compare(a, b), Ordering::Less, "{a} < {b}");
            assert_eq!(compare(b, a), Ordering::Greater, "{b} > {a}");
        };
        less("0.2.7", "0.2.23");
        less("1.9", "1.10.0");
        less("1.0.0-rc.9", "1.0.0-rc.10");
        less("1.0.0-alpha", "1.0.0");
        less("v0.1.0", "v0.17.0");
        less("2.0", "2.0.post1");
        less("2.0rc1", "2.0");
        less("4.17.15", "4.17.21");
        assert_eq!(compare("1.0", "1.0.0"), Ordering::Equal);
        assert_eq!(compare("v1.2.3", "1.2.3"), Ordering::Equal);
    }
}

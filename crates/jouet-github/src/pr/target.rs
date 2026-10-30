//! Naming a PR: a number, a URL, `owner/repo#123` or a branch, and the
//! repository a bare number or branch belongs to.

use std::fmt;
use std::process::Command;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Repo {
    pub owner: String,
    pub name: String,
}

impl Repo {
    /// Parses `owner/name`.
    pub fn parse(s: &str) -> Option<Self> {
        let (owner, name) = s.split_once('/')?;
        let valid = |p: &str| !p.is_empty() && !p.contains(['/', '#', ' ', ':']);
        (valid(owner) && valid(name)).then(|| Self {
            owner: owner.into(),
            name: name.into(),
        })
    }
}

impl fmt::Display for Repo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.owner, self.name)
    }
}

/// A PR argument as written, before a repository is chosen for it.
#[derive(Debug, PartialEq, Eq)]
pub enum Arg {
    Number(u64),
    Branch(String),
    InRepo(Repo, u64),
}

impl Arg {
    pub fn parse(s: &str) -> Self {
        if let Some(n) = number(s.strip_prefix('#').unwrap_or(s)) {
            return Self::Number(n);
        }
        if let Some((repo, n)) = s.split_once('#')
            && let (Some(repo), Some(n)) = (Repo::parse(repo), number(n))
        {
            return Self::InRepo(repo, n);
        }
        if let Some((_, rest)) = s.split_once("://") {
            // https://github.com/OWNER/REPO/pull/123[/files][?query][#fragment]
            let path = rest.split(['?', '#']).next().unwrap_or_default();
            let parts: Vec<&str> = path.split('/').collect();
            if let [_host, owner, name, "pull", n, ..] = parts.as_slice()
                && let (Some(repo), Some(n)) = (Repo::parse(&format!("{owner}/{name}")), number(n))
            {
                return Self::InRepo(repo, n);
            }
        }
        Self::Branch(s.to_string())
    }
}

/// A PR to look up, with its repository settled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Number(Repo, u64),
    Branch(Repo, String),
}

impl fmt::Display for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Number(repo, n) => write!(f, "{repo}#{n}"),
            Self::Branch(repo, branch) => write!(f, "{repo}:{branch}"),
        }
    }
}

fn number(s: &str) -> Option<u64> {
    (!s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
        .then(|| s.parse().ok())
        .flatten()
        .filter(|&n| n > 0)
}

/// The GitHub repository of this checkout's remotes, chosen in the order gh
/// uses: a remote marked by `gh repo set-default`, then `upstream`, `github`,
/// `origin`, then the rest as git lists them.
pub fn repo_from_git() -> Option<Repo> {
    let out = Command::new("git")
        .args(["config", "--get-regexp", r"^remote\..*\.(url|gh-resolved)$"])
        .output()
        .ok()?;
    pick_remote(&String::from_utf8_lossy(&out.stdout))
}

/// The GitHub repository named by `git config --get-regexp
/// '^remote\..*\.(url|gh-resolved)$'` output, chosen as [`repo_from_git`]
/// chooses, for a caller that ran git in another directory.
pub fn pick_remote(config: &str) -> Option<Repo> {
    let mut remotes: Vec<(String, Repo)> = Vec::new();
    let mut default = None;
    for line in config.lines() {
        let Some((key, value)) = line.split_once(' ') else {
            continue;
        };
        let Some(key) = key.strip_prefix("remote.") else {
            continue;
        };
        if let Some(name) = key.strip_suffix(".url") {
            if let Some(repo) = github_repo(value) {
                remotes.push((name.to_string(), repo));
            }
        } else if let Some(name) = key.strip_suffix(".gh-resolved") {
            // `base` marks the remote itself; anything else names a repo.
            default = Some(if value == "base" {
                Err(name.to_string())
            } else {
                Ok(value.to_string())
            });
        }
    }
    match default {
        Some(Ok(repo)) => {
            if let Some(repo) = Repo::parse(&repo) {
                return Some(repo);
            }
        }
        Some(Err(name)) => {
            if let Some((_, repo)) = remotes.iter().find(|(n, _)| *n == name) {
                return Some(repo.clone());
            }
        }
        None => {}
    }
    let rank = |name: &str| match name {
        "upstream" => 0,
        "github" => 1,
        "origin" => 2,
        _ => 3,
    };
    remotes.sort_by_key(|(name, _)| rank(name));
    remotes.into_iter().next().map(|(_, repo)| repo)
}

/// `owner/name` from a GitHub remote URL in any of git's forms.
fn github_repo(url: &str) -> Option<Repo> {
    let rest = if let Some((_, rest)) = url.split_once("://") {
        // https://github.com/o/r, ssh://git@github.com:22/o/r
        let (host, path) = rest.split_once('/')?;
        let host = host.rsplit('@').next()?.split(':').next()?;
        is_github(host).then_some(path)?
    } else {
        // git@github.com:o/r
        let (host, path) = url.split_once(':')?;
        is_github(host.rsplit('@').next()?).then_some(path)?
    };
    let path = rest.trim_end_matches('/');
    Repo::parse(path.strip_suffix(".git").unwrap_or(path))
}

fn is_github(host: &str) -> bool {
    matches!(host, "github.com" | "www.github.com" | "ssh.github.com")
}

/// The branch checked out in this directory, if any.
pub fn current_branch() -> Option<String> {
    let out = Command::new("git")
        .args(["symbolic-ref", "--quiet", "--short", "HEAD"])
        .output()
        .ok()?;
    let branch = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && !branch.is_empty()).then_some(branch)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo(s: &str) -> Repo {
        Repo::parse(s).unwrap()
    }

    #[test]
    fn arguments() {
        assert_eq!(Arg::parse("15002"), Arg::Number(15002));
        assert_eq!(Arg::parse("#15002"), Arg::Number(15002));
        assert_eq!(
            Arg::parse("jdx/mise#15002"),
            Arg::InRepo(repo("jdx/mise"), 15002)
        );
        for url in [
            "https://github.com/jdx/mise/pull/15002",
            "https://github.com/jdx/mise/pull/15002/",
            "https://github.com/jdx/mise/pull/15002/files?w=1",
            "https://github.com/jdx/mise/pull/15002#discussion_r1",
        ] {
            assert_eq!(
                Arg::parse(url),
                Arg::InRepo(repo("jdx/mise"), 15002),
                "{url}"
            );
        }
        assert_eq!(
            Arg::parse("feat/pr-status"),
            Arg::Branch("feat/pr-status".into())
        );
        assert_eq!(Arg::parse("0"), Arg::Branch("0".into()));
        assert_eq!(
            Arg::parse("https://github.com/jdx/mise/issues/1"),
            Arg::Branch("https://github.com/jdx/mise/issues/1".into())
        );
    }

    #[test]
    fn repos() {
        assert_eq!(repo("jdx/mise").to_string(), "jdx/mise");
        for bad in ["jdx", "/mise", "jdx/", "jdx/mise/x", "jdx/mi se"] {
            assert_eq!(Repo::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn remote_urls() {
        for url in [
            "git@github.com:jdx/mise.git",
            "git@github.com:jdx/mise",
            "https://github.com/jdx/mise.git",
            "https://github.com/jdx/mise/",
            "https://user@github.com/jdx/mise",
            "ssh://git@github.com/jdx/mise.git",
            "ssh://git@ssh.github.com:443/jdx/mise.git",
        ] {
            assert_eq!(github_repo(url), Some(repo("jdx/mise")), "{url}");
        }
        assert_eq!(github_repo("git@gitlab.com:jdx/mise.git"), None);
        assert_eq!(github_repo("/home/me/src/mise"), None);
    }

    #[test]
    fn remotes_are_chosen_in_ghs_order() {
        let config = "remote.fork.url git@github.com:me/mise.git\n\
                      remote.origin.url git@github.com:jdx/mise.git\n\
                      remote.upstream.url https://gitlab.com/jdx/mise\n";
        // A non-GitHub `upstream` is skipped, and `origin` beats `fork`.
        assert_eq!(pick_remote(config), Some(repo("jdx/mise")));
        let config = "remote.origin.url git@github.com:me/mise.git\n\
                      remote.upstream.url git@github.com:jdx/mise.git\n";
        assert_eq!(pick_remote(config), Some(repo("jdx/mise")));
        let config = "remote.origin.url git@github.com:jdx/mise.git\n\
                      remote.fork.url git@github.com:me/mise.git\n\
                      remote.fork.gh-resolved base\n";
        assert_eq!(pick_remote(config), Some(repo("me/mise")));
        let config = "remote.origin.url git@github.com:me/mise.git\n\
                      remote.origin.gh-resolved jdx/mise\n";
        assert_eq!(pick_remote(config), Some(repo("jdx/mise")));
        assert_eq!(pick_remote(""), None);
    }
}

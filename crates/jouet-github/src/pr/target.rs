//! Naming a PR: a number, a URL, `owner/repo#123` or a branch, and the
//! repository a bare number or branch belongs to.
//!
//! A repository is on GitHub unless it names a host: `gitlab.com/group/sub/project`,
//! `codeberg.org/owner/repo`, or the host of the checkout's remote. On
//! GitLab the owner is a group path and can hold slashes, so a repository
//! is split at its last `/`, never its first.

use crate::forge::{self, Forge};
use std::fmt;
use std::process::Command;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Repo {
    /// A user or organisation, or on GitLab a group path such as
    /// `group/subgroup`.
    pub owner: String,
    pub name: String,
    /// The host of a repository that is not on github.com, such as
    /// `gitlab.com` or `codeberg.org`. `None` is GitHub.
    pub host: Option<String>,
}

impl Repo {
    /// Parses `OWNER/NAME`, a GitHub repository, or `HOST/OWNER/NAME`, where
    /// a first part with a dot or a colon is the host.
    pub fn parse(s: &str) -> Option<Self> {
        let (first, rest) = s.split_once('/')?;
        if first.contains(['.', ':'])
            && let Some(repo) = Self::on(first, rest)
        {
            return Some(repo);
        }
        let (owner, name) = (first, rest);
        (valid(owner) && valid(name)).then(|| Self {
            owner: owner.into(),
            name: name.into(),
            host: None,
        })
    }

    /// The repository at `path` on `host`. Only a GitLab path (or one on a
    /// host jouet does not know yet) can have more than two parts.
    pub fn on(host: &str, path: &str) -> Option<Self> {
        let host = forge::normalize(host);
        if host.is_empty() || host.contains(['/', '@', ' ', '#']) {
            return None;
        }
        let path = path.trim_matches('/');
        let path = path.strip_suffix(".git").unwrap_or(path);
        let known = forge::of_host(&host);
        if known == Some(Forge::Entire) {
            return Self::on_entire(host, path);
        }
        let (owner, name) = path.rsplit_once('/')?;
        let nested = owner.contains('/');
        if (nested && matches!(known, Some(Forge::GitHub | Forge::Forgejo)))
            || !owner.split('/').all(valid)
            || !valid(name)
        {
            return None;
        }
        Some(Self {
            owner: owner.into(),
            name: name.into(),
            host: (known != Some(Forge::GitHub)).then_some(host),
        })
    }

    /// A path on Entire: `et/PROJECT/REPO` (or `PROJECT/REPO`, as
    /// [`Repo::qualified`] writes it) is hosted on Entire, and
    /// `gh/OWNER/REPO` is the GitHub repository Entire mirrors.
    fn on_entire(host: String, path: &str) -> Option<Self> {
        let parts: Vec<&str> = path.split('/').collect();
        let (owner, name, host) = match parts.as_slice() {
            ["gh", owner, name] => (*owner, *name, None),
            ["et", owner, name] | [owner, name] => (*owner, *name, Some(host)),
            _ => return None,
        };
        (valid(owner) && valid(name)).then(|| Self {
            owner: owner.into(),
            name: name.into(),
            host,
        })
    }

    /// The forge the repository is on; `None` for a host jouet does not
    /// know.
    pub fn forge(&self) -> Option<Forge> {
        match &self.host {
            None => Some(Forge::GitHub),
            Some(host) => forge::of_host(host),
        }
    }

    /// `HOST/OWNER/NAME`, or `OWNER/NAME` on GitHub: what [`Repo::parse`]
    /// reads back.
    pub fn qualified(&self) -> String {
        match &self.host {
            None => self.to_string(),
            Some(host) => format!("{host}/{self}"),
        }
    }

    /// A name for files kept about the repository: `OWNER-NAME` on GitHub,
    /// `HOST-OWNER-NAME` elsewhere, with a group path's slashes as dashes.
    pub fn slug(&self) -> String {
        let slug = match &self.host {
            None => format!("{}-{}", self.owner, self.name),
            Some(host) => format!("{host}-{}-{}", self.owner, self.name),
        };
        slug.replace(['/', ':'], "-")
    }
}

fn valid(part: &str) -> bool {
    !part.is_empty() && !part.contains(['/', '#', '!', ' ', ':'])
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
    /// Reads `123`, `#123`, GitLab's `!123`, `REPO#123` (or `REPO!123`), a
    /// PR or merge request URL, or else a branch.
    pub fn parse(s: &str) -> Self {
        let bare = s
            .strip_prefix('#')
            .or_else(|| s.strip_prefix('!'))
            .unwrap_or(s);
        if let Some(n) = number(bare) {
            return Self::Number(n);
        }
        if !s.contains("://")
            && let Some((repo, n)) = s.rsplit_once(['#', '!'])
            && let (Some(repo), Some(n)) = (Repo::parse(repo), number(n))
        {
            return Self::InRepo(repo, n);
        }
        if let Some((_, rest)) = s.split_once("://")
            && let Some((repo, n)) = pr_url(rest)
        {
            return Self::InRepo(repo, n);
        }
        Self::Branch(s.to_string())
    }
}

/// The repository and number in a PR URL, after its scheme:
/// `HOST/OWNER/NAME/pull/N` (GitHub), `HOST/OWNER/NAME/pulls/N` (Forgejo),
/// `HOST/et/PROJECT/NAME/trails/N` (Entire) or
/// `HOST/GROUP/.../NAME/-/merge_requests/N` (GitLab), each with anything
/// after it.
fn pr_url(rest: &str) -> Option<(Repo, u64)> {
    let path = rest.split(['?', '#']).next()?;
    let (host, path) = path.split_once('/')?;
    let host = host.rsplit('@').next()?;
    let parts: Vec<&str> = path.split('/').collect();
    let (repo, n) = if let Some(dash) = parts.iter().position(|p| *p == "-") {
        if parts.get(dash + 1) != Some(&"merge_requests") {
            return None;
        }
        (parts[..dash].join("/"), parts.get(dash + 2)?)
    } else {
        match parts.as_slice() {
            [owner, name, "pull" | "pulls", n, ..] => (format!("{owner}/{name}"), n),
            // An Entire trail; a GitHub repository's trail is not its PR.
            ["et", owner, name, "trails", n, ..] => (format!("et/{owner}/{name}"), n),
            _ => return None,
        }
    };
    Some((Repo::on(host, &repo)?, number(n)?))
}

/// A PR to look up, with its repository settled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Number(Repo, u64),
    Branch(Repo, String),
}

impl Target {
    pub fn repo(&self) -> &Repo {
        match self {
            Self::Number(repo, _) | Self::Branch(repo, _) => repo,
        }
    }
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

/// The repository of this checkout's remotes, on any forge jouet knows,
/// chosen in the order gh uses: a remote marked by `gh repo set-default`,
/// then `upstream`, `github`, `origin`, then the rest as git lists them,
/// with GitHub remotes before those on other forges.
pub fn repo_from_git() -> Option<Repo> {
    let out = Command::new("git")
        .args(["config", "--get-regexp", r"^remote\..*\.(url|gh-resolved)$"])
        .output()
        .ok()?;
    pick_remote(&String::from_utf8_lossy(&out.stdout))
}

/// The repository named by `git config --get-regexp
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
            if let Some(repo) = remote_repo(value) {
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
    // GitHub first, so a checkout that also has a GitLab or Forgejo mirror
    // keeps reading GitHub.
    remotes.sort_by_key(|(name, repo)| (repo.host.is_some(), rank(name)));
    remotes.into_iter().next().map(|(_, repo)| repo)
}

/// The repository a remote URL names, in any of git's forms, when its host
/// runs a forge jouet knows.
fn remote_repo(url: &str) -> Option<Repo> {
    let (host, path) = if let Some((scheme, rest)) = url.split_once("://") {
        // https://gitlab.com/g/s/r, ssh://git@github.com:22/o/r
        let (authority, path) = rest.split_once('/')?;
        let host = authority.rsplit('@').next()?;
        // An HTTP port is the web server's; an SSH port says nothing of it.
        let host = if scheme.starts_with("http") {
            host
        } else {
            host.split(':').next()?
        };
        (host, path)
    } else {
        // git@github.com:o/r
        let (host, path) = url.split_once(':')?;
        (host.rsplit('@').next()?, path)
    };
    forge::of_host(host)?;
    Repo::on(host, path.trim_end_matches('/'))
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
            assert_eq!(remote_repo(url), Some(repo("jdx/mise")), "{url}");
        }
        assert_eq!(remote_repo("/home/me/src/mise"), None);
        assert_eq!(remote_repo("git@git.example.com:jdx/mise.git"), None);
    }

    #[test]
    fn gitlab_group_paths_split_at_the_last_slash() {
        let nested = Repo::parse("gitlab.com/group/sub/project").unwrap();
        assert_eq!(nested.owner, "group/sub");
        assert_eq!(nested.name, "project");
        assert_eq!(nested.host.as_deref(), Some("gitlab.com"));
        assert_eq!(nested.to_string(), "group/sub/project");
        assert_eq!(nested.qualified(), "gitlab.com/group/sub/project");
        assert_eq!(nested.slug(), "gitlab.com-group-sub-project");
        for url in [
            "git@gitlab.com:group/sub/project.git",
            "https://gitlab.com/group/sub/project",
            "ssh://git@altssh.gitlab.com:443/group/sub/project.git",
        ] {
            assert_eq!(remote_repo(url).as_ref(), Some(&nested), "{url}");
        }
        for arg in [
            "https://gitlab.com/group/sub/project/-/merge_requests/12",
            "https://gitlab.com/group/sub/project/-/merge_requests/12/diffs?w=1",
            "gitlab.com/group/sub/project!12",
            "gitlab.com/group/sub/project#12",
        ] {
            assert_eq!(Arg::parse(arg), Arg::InRepo(nested.clone(), 12), "{arg}");
        }
        assert_eq!(Arg::parse("!12"), Arg::Number(12));
        // Only GitLab nests groups.
        assert_eq!(Repo::parse("codeberg.org/a/b/c"), None);
        assert_eq!(Repo::parse("github.com/a/b/c"), None);
    }

    #[test]
    fn forgejo_and_github_hosts() {
        let forgejo = Repo::parse("codeberg.org/forgejo/forgejo").unwrap();
        assert_eq!(forgejo.forge(), Some(Forge::Forgejo));
        assert_eq!(
            Arg::parse("https://codeberg.org/forgejo/forgejo/pulls/7/files"),
            Arg::InRepo(forgejo.clone(), 7)
        );
        assert_eq!(
            remote_repo("git@codeberg.org:forgejo/forgejo.git"),
            Some(forgejo)
        );
        // github.com named as a host is the same GitHub repository.
        assert_eq!(Repo::parse("github.com/jdx/mise"), Some(repo("jdx/mise")));
        assert_eq!(repo("jdx/mise").slug(), "jdx-mise");
        // A host jouet does not know parses, and has no forge.
        let unknown = Repo::parse("git.example.com/a/b").unwrap();
        assert_eq!(unknown.forge(), None);
    }

    #[test]
    fn entire_hosted_repositories_and_mirrors() {
        let native = Repo::parse("entire.com/jdx/jouet-entire-forge-test").unwrap();
        assert_eq!(native.forge(), Some(Forge::Entire));
        assert_eq!(native.owner, "jdx");
        assert_eq!(native.qualified(), "entire.com/jdx/jouet-entire-forge-test");
        for url in [
            "entire://aws-us-east-2.entire.io/et/jdx/jouet-entire-forge-test",
            "entire://aws-eu-central-1.entire.io/et/jdx/jouet-entire-forge-test.git",
        ] {
            assert_eq!(remote_repo(url).as_ref(), Some(&native), "{url}");
        }
        assert_eq!(
            Arg::parse("https://entire.com/et/jdx/jouet-entire-forge-test/trails/3/changes"),
            Arg::InRepo(native.clone(), 3)
        );
        assert_eq!(
            Arg::parse("entire.com/jdx/jouet-entire-forge-test#3"),
            Arg::InRepo(native, 3)
        );
        // A GitHub repository served through Entire is still GitHub's, and
        // its trail numbers are not PR numbers.
        assert_eq!(
            remote_repo("entire://aws-us-east-2.entire.io/gh/jdx/mise"),
            Some(repo("jdx/mise"))
        );
        assert!(matches!(
            Arg::parse("https://entire.com/gh/jdx/mise/trails/424"),
            Arg::Branch(_)
        ));
        assert_eq!(Repo::parse("entire.com/et/a/b/c"), None);
    }

    #[test]
    fn remotes_on_other_forges_are_chosen_too() {
        let config = "remote.origin.url git@gitlab.com:group/sub/project.git\n";
        assert_eq!(
            pick_remote(config),
            Repo::parse("gitlab.com/group/sub/project")
        );
    }

    #[test]
    fn remotes_are_chosen_in_ghs_order() {
        let config = "remote.fork.url git@github.com:me/mise.git\n\
                      remote.origin.url git@github.com:jdx/mise.git\n\
                      remote.upstream.url https://gitlab.com/jdx/mise\n";
        // A GitHub `origin` beats a GitLab `upstream`, and `origin` beats `fork`.
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

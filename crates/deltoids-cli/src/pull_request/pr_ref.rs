//! Pull request references as people write them, and the remote URLs
//! that point at a pull request's repository.

use std::fmt;
use std::str::FromStr;

/// A pull request named on the command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PrRef {
    /// `github.com` unless a URL names another host.
    pub(crate) host: String,
    pub(crate) owner: String,
    pub(crate) repo: String,
    pub(crate) number: u64,
}

const DEFAULT_HOST: &str = "github.com";

impl FromStr for PrRef {
    type Err = String;

    /// `owner/repo#123`, or a pull request URL such as
    /// `https://github.com/owner/repo/pull/123/files`.
    fn from_str(text: &str) -> Result<Self, String> {
        let text = text.trim();
        let parsed = if text.contains("://") {
            from_url(text)
        } else {
            from_short(text)
        };
        parsed.ok_or_else(|| {
            format!("not a pull request: {text} (expected owner/repo#123 or a pull request URL)")
        })
    }
}

impl fmt::Display for PrRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.host != DEFAULT_HOST {
            write!(f, "{}/", self.host)?;
        }
        write!(f, "{}/{}#{}", self.owner, self.repo, self.number)
    }
}

fn from_short(text: &str) -> Option<PrRef> {
    let (name, number) = text.split_once('#')?;
    let (owner, repo) = name.split_once('/')?;
    pr_ref(DEFAULT_HOST, owner, repo, number)
}

fn from_url(text: &str) -> Option<PrRef> {
    let url = url::Url::parse(text).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    let mut segments = url.path_segments()?;
    let owner = segments.next()?;
    let repo = segments.next()?;
    if segments.next()? != "pull" {
        return None;
    }
    pr_ref(url.host_str()?, owner, repo, segments.next()?)
}

fn pr_ref(host: &str, owner: &str, repo: &str, number: &str) -> Option<PrRef> {
    let valid = |part: &str| !part.is_empty() && !part.contains(['/', ' ', '#']);
    if !valid(owner) || !valid(repo) {
        return None;
    }
    Some(PrRef {
        host: host.to_ascii_lowercase(),
        owner: owner.to_string(),
        repo: repo.to_string(),
        number: number.parse().ok().filter(|n| *n > 0)?,
    })
}

/// Whether the remote `url` names the pull request's repository. Hosts
/// are not compared, so SSH host aliases (`github-work:owner/repo`)
/// match too. Owner and repository compare without case, as GitHub does.
pub(super) fn remote_names(url: &str, pr: &PrRef) -> bool {
    remote_path(url).is_some_and(|(owner, repo)| {
        owner.eq_ignore_ascii_case(&pr.owner) && repo.eq_ignore_ascii_case(&pr.repo)
    })
}

/// The `owner` and `repo` of `https://host/owner/repo.git`,
/// `ssh://git@host/owner/repo`, or `git@host:owner/repo.git`.
fn remote_path(url: &str) -> Option<(String, String)> {
    let path = match url.split_once("://") {
        Some((_, rest)) => rest.split_once('/')?.1,
        None => url.split_once(':')?.1,
    };
    let path = path.trim_end_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let (owner, repo) = path.rsplit_once('/')?;
    let owner = owner.rsplit('/').next()?;
    (!owner.is_empty() && !repo.is_empty()).then(|| (owner.to_string(), repo.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pr(host: &str, owner: &str, repo: &str, number: u64) -> PrRef {
        PrRef {
            host: host.to_string(),
            owner: owner.to_string(),
            repo: repo.to_string(),
            number,
        }
    }

    #[test]
    fn parses_the_short_form() {
        assert_eq!(
            "acme/widgets#12".parse::<PrRef>(),
            Ok(pr("github.com", "acme", "widgets", 12))
        );
    }

    #[test]
    fn parses_pull_request_urls() {
        for url in [
            "https://github.com/acme/widgets/pull/12",
            "https://github.com/acme/widgets/pull/12/",
            "https://github.com/acme/widgets/pull/12/files",
            "https://github.com/acme/widgets/pull/12/commits?w=1#diff",
            "  http://GitHub.com/acme/widgets/pull/12  ",
        ] {
            assert_eq!(
                url.parse::<PrRef>(),
                Ok(pr("github.com", "acme", "widgets", 12)),
                "{url}"
            );
        }
    }

    #[test]
    fn keeps_an_enterprise_host() {
        let parsed: PrRef = "https://git.corp.example/acme/widgets/pull/3"
            .parse()
            .unwrap();
        assert_eq!(parsed, pr("git.corp.example", "acme", "widgets", 3));
        assert_eq!(parsed.to_string(), "git.corp.example/acme/widgets#3");
    }

    #[test]
    fn rejects_what_is_not_a_pull_request() {
        for text in [
            "",
            "acme/widgets",
            "acme/widgets#",
            "acme/widgets#0",
            "acme#12",
            "/widgets#12",
            "acme/widgets#twelve",
            "https://github.com/acme/widgets",
            "https://github.com/acme/widgets/issues/12",
            "https://github.com/acme/widgets/pull/abc",
            "ssh://github.com/acme/widgets/pull/12",
        ] {
            assert!(text.parse::<PrRef>().is_err(), "{text:?} parsed");
        }
    }

    #[test]
    fn remotes_name_the_repository_in_every_url_form() {
        let widgets = pr("github.com", "acme", "widgets", 1);
        for url in [
            "https://github.com/acme/widgets.git",
            "https://github.com/acme/widgets",
            "https://github.com/acme/widgets/",
            "https://token@github.com/Acme/Widgets.git",
            "ssh://git@github.com/acme/widgets.git",
            "ssh://git@github.com:22/acme/widgets",
            "git@github.com:acme/widgets.git",
            "github-work:acme/widgets",
        ] {
            assert!(remote_names(url, &widgets), "{url}");
        }
    }

    #[test]
    fn remotes_of_other_repositories_do_not_match() {
        let widgets = pr("github.com", "acme", "widgets", 1);
        for url in [
            "https://github.com/me/widgets.git",
            "https://github.com/acme/widgets-fork.git",
            "git@github.com:acme/gadgets.git",
            "/srv/git/widgets.git",
            "widgets",
        ] {
            assert!(!remote_names(url, &widgets), "{url}");
        }
    }
}

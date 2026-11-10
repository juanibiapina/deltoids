//! What the pull request is, from GitHub's REST API through `gh api`.

use std::ffi::OsStr;
use std::process::{Command, Stdio};

use deltoids::git;
use serde::Deserialize;

use super::{PrRef, PullRequestInfo};

/// The pull request, and its two ends.
pub(super) struct Pull {
    pub(super) info: PullRequestInfo,
    /// The head commit's full id.
    pub(super) head: String,
    /// The base branch's commit the pull request was last compared with.
    pub(super) base: String,
    /// Where to fetch the base repository from.
    pub(super) clone_url: String,
}

/// What GitHub's three-dot comparison of a pull request says.
pub(super) struct Compared {
    pub(super) merge_base: String,
    /// Newest first.
    pub(super) commits: Vec<git::Commit>,
}

#[derive(Deserialize)]
struct PullJson {
    title: String,
    body: Option<String>,
    user: Option<User>,
    #[serde(default)]
    html_url: String,
    head: HeadJson,
    base: BaseJson,
}

#[derive(Deserialize)]
struct User {
    login: String,
}

#[derive(Deserialize)]
struct HeadJson {
    sha: String,
    #[serde(default)]
    label: String,
}

#[derive(Deserialize)]
struct BaseJson {
    sha: String,
    #[serde(rename = "ref", default)]
    name: String,
    repo: RepoJson,
}

#[derive(Deserialize)]
struct RepoJson {
    clone_url: String,
}

#[derive(Deserialize)]
struct Sha {
    sha: String,
}

#[derive(Deserialize)]
struct ComparePage {
    merge_base_commit: Sha,
    commits: Vec<CommitJson>,
}

#[derive(Deserialize)]
struct CommitJson {
    sha: String,
    commit: CommitBody,
}

#[derive(Deserialize)]
struct CommitBody {
    message: String,
    author: Option<Person>,
}

#[derive(Deserialize)]
struct Person {
    name: String,
    email: String,
    date: String,
}

pub(super) fn pull(gh: &OsStr, pr: &PrRef) -> Result<Pull, String> {
    let path = format!("repos/{}/{}/pulls/{}", pr.owner, pr.repo, pr.number);
    let json: PullJson = parse(&api(gh, pr, &[&path])?, &path)?;
    Ok(Pull {
        info: PullRequestInfo {
            number: pr.number,
            title: json.title,
            author: json.user.map(|user| user.login).unwrap_or_default(),
            url: json.html_url,
            base: json.base.name,
            head: json.head.label,
            body: json.body.unwrap_or_default(),
        },
        head: json.head.sha,
        base: json.base.sha,
        clone_url: json.base.repo.clone_url,
    })
}

/// GitHub's comparison of `base...head` as it answers, every page of
/// commits included: unpaged, GitHub lists at most 250.
pub(super) fn compare_pages(
    gh: &OsStr,
    pr: &PrRef,
    base: &str,
    head: &str,
) -> Result<Vec<u8>, String> {
    let path = compare_path(pr, base, head);
    let pages = api(gh, pr, &["--paginate", "--slurp", &path])?;
    parse_compare(&pages, &path)?;
    Ok(pages)
}

/// Read what [`compare_pages`] returned.
pub(super) fn compare(
    pages: &[u8],
    pr: &PrRef,
    base: &str,
    head: &str,
) -> Result<Compared, String> {
    parse_compare(pages, &compare_path(pr, base, head))
}

fn compare_path(pr: &PrRef, base: &str, head: &str) -> String {
    format!(
        "repos/{}/{}/compare/{base}...{head}?per_page=100",
        pr.owner, pr.repo
    )
}

fn parse_compare(body: &[u8], path: &str) -> Result<Compared, String> {
    let pages: Vec<ComparePage> = parse(body, path)?;
    let merge_base = pages
        .first()
        .map(|page| page.merge_base_commit.sha.clone())
        .ok_or_else(|| format!("gh api {path}: empty response"))?;
    let mut commits: Vec<git::Commit> = pages
        .into_iter()
        .flat_map(|page| page.commits)
        .map(commit)
        .collect();
    commits.reverse();
    Ok(Compared {
        merge_base,
        commits,
    })
}

fn commit(json: CommitJson) -> git::Commit {
    let message = json.commit.message;
    let (author, email, time) = match json.commit.author {
        Some(person) => {
            let time = person
                .date
                .parse::<jiff::Timestamp>()
                .map_or(0, |t| t.as_second());
            (person.name, person.email, time)
        }
        None => Default::default(),
    };
    git::Commit {
        short_id: json.sha.chars().take(7).collect(),
        id: json.sha,
        summary: message.lines().next().unwrap_or_default().to_string(),
        message,
        author,
        email,
        time,
    }
}

fn parse<T: serde::de::DeserializeOwned>(body: &[u8], path: &str) -> Result<T, String> {
    serde_json::from_slice(body).map_err(|err| format!("gh api {path}: unexpected response: {err}"))
}

/// Run `gh api args` against the pull request's host and return stdout.
fn api(gh: &OsStr, pr: &PrRef, args: &[&str]) -> Result<Vec<u8>, String> {
    let mut command = Command::new(gh);
    command.arg("api");
    if pr.host != "github.com" {
        command.args(["--hostname", &pr.host]);
    }
    let output = command
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|err| match err.kind() {
            std::io::ErrorKind::NotFound => {
                "gh is not installed. Install the GitHub CLI (https://cli.github.com) and run gh auth login.".to_string()
            }
            _ => format!("gh: {err}"),
        })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "gh api {}: {}",
            args.last().unwrap_or(&""),
            stderr.trim()
        ));
    }
    Ok(output.stdout)
}

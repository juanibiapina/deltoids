//! The local repository a pull request's commits are read from, through
//! the git CLI: a clone the user already has, or a cache repository.

use std::ffi::OsStr;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use super::PrRef;
use super::pr_ref::remote_names;

/// A repository holding a pull request's commits, and the remote that
/// names the pull request's repository there.
pub(super) struct Local {
    pub(super) dir: PathBuf,
    remote: String,
    kind: Kind,
}

enum Kind {
    /// The user's clone: fetches are whole, so the clone stays a normal
    /// clone, and touch no ref.
    Clone,
    /// The cache: fetches are shallow and without blobs, and authenticate
    /// with this credential helper.
    Cache { helper: String },
}

impl Local {
    /// The clone of `pr`'s repository that contains `cwd`, if any.
    pub(super) fn clone_of(pr: &PrRef, cwd: &Path) -> Option<Self> {
        let urls = git(
            cwd,
            &["config", "--local", "--get-regexp", r"^remote\..*\.url$"],
        )
        .ok()?;
        urls.lines().find_map(|line| {
            let (key, url) = line.split_once(' ')?;
            let remote = key.strip_prefix("remote.")?.strip_suffix(".url")?;
            remote_names(url, pr).then(|| Self {
                dir: cwd.to_path_buf(),
                remote: remote.to_string(),
                kind: Kind::Clone,
            })
        })
    }

    /// The bare cache repository for `pr`'s repository under `root`,
    /// made on first use, fetching from `url` with `gh` as the
    /// credential helper.
    pub(super) fn cache(root: &Path, pr: &PrRef, url: &str, gh: &OsStr) -> Result<Self, String> {
        let dir = root
            .join("deltoids")
            .join("github")
            .join(&pr.host)
            .join(&pr.owner)
            .join(format!("{}.git", pr.repo));
        if dir.join("HEAD").exists() {
            git(&dir, &["remote", "set-url", "origin", url])?;
        } else {
            std::fs::create_dir_all(&dir)
                .map_err(|err| format!("cannot create {}: {err}", dir.display()))?;
            git(&dir, &["init", "--quiet", "--bare"])?;
            git(&dir, &["remote", "add", "origin", url])?;
        }
        let helper = format!(
            "credential.helper=!{} auth git-credential",
            gh.to_string_lossy()
        );
        Ok(Self {
            dir,
            remote: "origin".to_string(),
            kind: Kind::Cache { helper },
        })
    }

    /// Whether this is the cache repository, which fetches speculatively.
    pub(super) fn is_cache(&self) -> bool {
        matches!(self.kind, Kind::Cache { .. })
    }

    /// Fetch the commits of `ids` the repository lacks, in one request.
    pub(super) fn fetch_commits(&self, ids: &[&str]) -> Result<(), String> {
        let mut missing: Vec<&str> = ids
            .iter()
            .copied()
            .filter(|id| !self.has_commit(id))
            .collect();
        missing.dedup();
        if missing.is_empty() {
            return Ok(());
        }
        let mut args = vec!["fetch", "--quiet", "--no-tags", "--no-write-fetch-head"];
        if self.is_cache() {
            args.extend(["--depth=1", "--filter=blob:none"]);
        }
        args.push(&self.remote);
        args.extend(missing);
        self.fetch(&args, None)
    }

    /// Point the cache's `HEAD` at `head`: the engine reads attributes
    /// from `HEAD`. A clone keeps its own.
    pub(super) fn point_head(&self, head: &str) -> Result<(), String> {
        if !self.is_cache() {
            return Ok(());
        }
        git(&self.dir, &["update-ref", "--no-deref", "HEAD", head]).map(drop)
    }

    /// In a partial repository, fetch in one request the blobs the diff
    /// of `base..tip` reads, and every `.gitattributes` of `HEAD`, which
    /// the engine reads a file's conversions from. Renames need no more:
    /// the engine pairs them among the added and deleted files' blobs.
    pub(super) fn fetch_diff_blobs(&self, base: &str, tip: &str) -> Result<(), String> {
        if !is_partial(&self.dir) {
            return Ok(());
        }
        let raw = git(
            &self.dir,
            &[
                "diff",
                "--raw",
                "--no-abbrev",
                "--no-renames",
                "-z",
                base,
                tip,
            ],
        )?;
        let tree = git(&self.dir, &["ls-tree", "-r", "-z", "HEAD"])?;
        let mut wanted = diff_blobs(&raw);
        wanted.extend(attribute_blobs(&tree));
        let missing = self.missing_objects(&wanted)?;
        if missing.is_empty() {
            return Ok(());
        }
        let args = [
            "-c",
            "fetch.negotiationAlgorithm=noop",
            "fetch",
            "--quiet",
            "--no-tags",
            "--no-write-fetch-head",
            "--recurse-submodules=no",
            "--filter=blob:none",
            "--stdin",
            &self.remote,
        ];
        self.fetch(&args, Some(&missing.join("\n")))
    }

    /// Run a fetch, authenticated with the cache's credential helper.
    fn fetch(&self, args: &[&str], input: Option<&str>) -> Result<(), String> {
        let mut all = Vec::new();
        if let Kind::Cache { helper } = &self.kind {
            all.extend(["-c", "credential.helper=", "-c", helper.as_str()]);
        }
        all.extend(args);
        run(&self.dir, &all, input, &[]).map(drop)
    }

    /// Whether the commit `id` is here. Never fetches it: in a partial
    /// repository, git would fetch a missing commit with its whole
    /// history.
    fn has_commit(&self, id: &str) -> bool {
        let object = format!("{id}^{{commit}}");
        let lazy = [("GIT_NO_LAZY_FETCH", "1")];
        run(&self.dir, &["cat-file", "-e", &object], None, &lazy).is_ok()
    }

    /// The objects among `ids` the repository lacks, found without
    /// fetching them one at a time from a promisor remote.
    fn missing_objects(&self, ids: &[String]) -> Result<Vec<String>, String> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let checked = run(
            &self.dir,
            &["cat-file", "--batch-check"],
            Some(&ids.join("\n")),
            &[("GIT_NO_LAZY_FETCH", "1")],
        )?;
        let mut missing: Vec<String> = checked
            .lines()
            .filter_map(|line| line.strip_suffix(" missing"))
            .map(str::to_string)
            .collect();
        missing.sort();
        missing.dedup();
        Ok(missing)
    }
}

/// The blob ids on both sides of `git diff --raw -z` output, without
/// submodule commits or the null id of an added or deleted side.
fn diff_blobs(raw: &str) -> Vec<String> {
    raw.split('\0')
        .filter_map(|field| field.strip_prefix(':'))
        .flat_map(|meta| {
            let parts: Vec<&str> = meta.split(' ').collect();
            let sides = [(parts.first(), parts.get(2)), (parts.get(1), parts.get(3))];
            sides
                .into_iter()
                .filter_map(|(mode, id)| Some((*mode?, *id?)))
                .filter(|(mode, id)| *mode != "160000" && !id.bytes().all(|b| b == b'0'))
                .map(|(_, id)| id.to_string())
                .collect::<Vec<_>>()
        })
        .collect()
}

/// The blob ids of every `.gitattributes` in `git ls-tree -r -z` output.
fn attribute_blobs(tree: &str) -> Vec<String> {
    tree.split('\0')
        .filter_map(|entry| {
            let (meta, path) = entry.split_once('\t')?;
            let is_attributes = path == ".gitattributes" || path.ends_with("/.gitattributes");
            let mut parts = meta.split(' ');
            let (_, kind, id) = (parts.next()?, parts.next()?, parts.next()?);
            (is_attributes && kind == "blob").then(|| id.to_string())
        })
        .collect()
}

fn is_partial(dir: &Path) -> bool {
    let promisor = git(
        dir,
        &[
            "config",
            "--local",
            "--get-regexp",
            r"^(remote\..*\.promisor|extensions\.partialclone)$",
        ],
    );
    promisor.is_ok_and(|lines| {
        lines.lines().any(|line| {
            line.split_once(' ')
                .is_some_and(|(_, value)| value != "false")
        })
    })
}

fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    run(dir, args, None, &[])
}

/// Run git in `dir` without a terminal prompt, with `input` on stdin and
/// `env` set, and return stdout.
fn run(
    dir: &Path,
    args: &[&str],
    input: Option<&str>,
    env: &[(&str, &str)],
) -> Result<String, String> {
    let mut child = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .envs(env.iter().copied())
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| format!("git: {err}"))?;
    if let (Some(input), Some(mut stdin)) = (input, child.stdin.take()) {
        stdin
            .write_all(format!("{input}\n").as_bytes())
            .map_err(|err| format!("git: {err}"))?;
    }
    let output = child
        .wait_with_output()
        .map_err(|err| format!("git: {err}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let command = args
            .iter()
            .find(|arg| !arg.starts_with('-') && !arg.contains('='));
        return Err(format!(
            "git {} failed: {}",
            command.unwrap_or(&""),
            stderr.trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_blobs_skip_null_sides_and_submodules() {
        let raw = format!(
            ":100644 100644 {a} {b} M\0src/a.rs\0:000000 100644 {z} {c} A\0new.rs\0:160000 160000 {d} {e} M\0vendor\0",
            a = "a".repeat(40),
            b = "b".repeat(40),
            c = "c".repeat(40),
            d = "d".repeat(40),
            e = "e".repeat(40),
            z = "0".repeat(40),
        );
        assert_eq!(
            diff_blobs(&raw),
            ["a".repeat(40), "b".repeat(40), "c".repeat(40)]
        );
    }

    #[test]
    fn attribute_blobs_find_every_gitattributes() {
        let tree = format!(
            "100644 blob {a}\t.gitattributes\0100644 blob {b}\tsrc/a.rs\0100644 blob {c}\tsub/.gitattributes\0100644 blob {d}\tnot.gitattributes\0",
            a = "a".repeat(40),
            b = "b".repeat(40),
            c = "c".repeat(40),
            d = "d".repeat(40),
        );
        assert_eq!(attribute_blobs(&tree), ["a".repeat(40), "c".repeat(40)]);
    }
}

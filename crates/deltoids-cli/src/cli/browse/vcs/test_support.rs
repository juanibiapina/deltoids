//! A `but` stand-in for tests that drive a GitButler workspace.

use std::fs;
use std::path::{Path, PathBuf};

use super::Branch;

pub(in crate::cli::browse) const FIXTURES: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/src/cli/browse/vcs/fixtures");

/// A current branch named `name` with no CLI ID and no listed commits.
pub(in crate::cli::browse) fn branch(name: &str) -> Branch {
    Branch {
        id: None,
        name: name.to_string(),
        current: true,
        summary: String::new(),
        time: 0,
        commits: Vec::new(),
        base: None,
        rev: format!("refs/heads/{name}"),
    }
}

/// Record a GitButler target in the repository's config at `dir`, as
/// `but` does when it adopts a repository. `HEAD` stays where it is.
pub(in crate::cli::browse) fn adopt(dir: &Path) {
    crate::test_git::TestRepo::open(dir).git(&[
        "config",
        "gitbutler.project.targetRef",
        "refs/remotes/origin/main",
    ]);
}

/// A recorded fixture's path.
pub(in crate::cli::browse) fn fixture(name: &str) -> PathBuf {
    Path::new(FIXTURES).join(name)
}

/// A `but` stand-in in `dir` that logs each command line to `but.log`,
/// answers `status` with the JSON at `status`, and `branch list` with
/// the recorded branch list.
#[cfg(unix)]
pub(in crate::cli::browse) fn stub_but(dir: &Path, status: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let script = dir.join("but");
    let log = dir.join("but.log");
    fs::write(
        &script,
        format!(
            "#!/bin/sh\nshift 3\necho \"$*\" >> '{log}'\ncase \"$1\" in\n  status) cat '{status}' ;;\n  branch) cat '{branches}' ;;\nesac\n",
            log = log.display(),
            status = status.display(),
            branches = fixture("but-branch-list.json").display(),
        ),
    )
    .unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    script
}

/// The command lines the stand-in in `dir` ran, in order.
pub(in crate::cli::browse) fn but_log(dir: &Path) -> Vec<String> {
    fs::read_to_string(dir.join("but.log"))
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
}

use git2::{Delta, Diff, DiffFindOptions, DiffFormat, DiffOptions, Repository};
use std::path::Path;
use std::time::Instant;

use deltoids::git as actual_git;
use actual_git::{FileStageStatus, StageChange};

#[derive(Debug)]
struct Row {
    old: Vec<u8>,
    new: Vec<u8>,
    change: Option<StageChange>,
}

fn patch_options() -> DiffOptions {
    let mut opts = DiffOptions::new();
    opts.include_untracked(true)
        .recurse_untracked_dirs(true)
        .show_untracked_content(true)
        .include_typechange(true);
    opts
}

fn print_patch(diff: &Diff<'_>) -> Result<String, git2::Error> {
    let mut out = String::new();
    diff.print(DiffFormat::Patch, |_, _, line| {
        if matches!(line.origin(), '+' | '-' | ' ') {
            out.push(line.origin());
        }
        if let Ok(s) = std::str::from_utf8(line.content()) {
            out.push_str(s);
        }
        true
    })?;
    Ok(out)
}

fn rows(diff: &Diff<'_>, staged: bool) -> Vec<Row> {
    diff.deltas().map(|delta| {
        let change = match delta.status() {
            Delta::Added | Delta::Copied | Delta::Untracked => {
                Some(if staged { StageChange::Added } else { StageChange::Untracked })
            }
            Delta::Modified => Some(StageChange::Modified),
            Delta::Deleted => Some(StageChange::Deleted),
            Delta::Renamed => Some(StageChange::Renamed),
            Delta::Typechange => Some(StageChange::TypeChanged),
            _ => None,
        };
        Row {
            old: delta.old_file().path_bytes().unwrap_or_default().to_vec(),
            new: delta.new_file().path_bytes().unwrap_or_default().to_vec(),
            change,
        }
    }).collect()
}

fn pair(staged: &Diff<'_>, unstaged: &Diff<'_>) -> Vec<FileStageStatus> {
    let insensitive = staged.is_sorted_icase() && unstaged.is_sorted_icase();
    let compare = |a: &[u8], b: &[u8]| {
        if insensitive {
            a.iter().map(u8::to_ascii_lowercase).cmp(b.iter().map(u8::to_ascii_lowercase))
        } else {
            a.cmp(b)
        }
    };
    let mut left = rows(staged, true);
    let mut right = rows(unstaged, false);
    left.sort_by(|a, b| compare(&a.new, &b.new));
    right.sort_by(|a, b| compare(&a.old, &b.old));
    let mut i = 0;
    let mut j = 0;
    let mut out = Vec::new();
    while i < left.len() || j < right.len() {
        let order = if i == left.len() { std::cmp::Ordering::Greater }
            else if j == right.len() { std::cmp::Ordering::Less }
            else { compare(&left[i].new, &right[j].old) };
        let (path, staged, unstaged) = match order {
            std::cmp::Ordering::Less => {
                let x = &left[i]; i += 1;
                (&x.new, x.change, None)
            }
            std::cmp::Ordering::Greater => {
                let y = &right[j]; j += 1;
                (&y.new, None, y.change)
            }
            std::cmp::Ordering::Equal => {
                let x = &left[i]; let y = &right[j]; i += 1; j += 1;
                (&x.new, x.change, y.change)
            }
        };
        if staged.is_some() || unstaged.is_some() {
            out.push(FileStageStatus { path: String::from_utf8_lossy(path).into_owned(), staged, unstaged });
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

fn source_fingerprint(diff: &Diff<'_>) -> Vec<String> {
    diff.deltas().map(|d| format!("{:?}:{:?}:{:?}:{:?}:{:?}:{:?}",
        d.status(), d.flags(), d.old_file().path_bytes(), d.new_file().path_bytes(),
        d.old_file().id(), d.new_file().id())).collect()
}

fn snapshot(repo: &Repository, check_source: bool, force_index: bool) -> Result<(String, Vec<FileStageStatus>), git2::Error> {
    let head = match repo.head() {
        Ok(head) => Some(head.peel_to_tree()?),
        Err(error) if matches!(error.code(), git2::ErrorCode::UnbornBranch | git2::ErrorCode::NotFound) => None,
        Err(error) => return Err(error),
    };
    let mut index = repo.index()?;
    index.read(force_index)?;
    let mut patch = repo.diff_tree_to_index(head.as_ref(), Some(&index), Some(&mut patch_options()))?;
    let mut work = repo.diff_index_to_workdir(Some(&index), Some(&mut patch_options()))?;
    let before = if check_source { Some(source_fingerprint(&work)) } else { None };
    patch.merge(&work)?;
    let mut find = DiffFindOptions::new();
    find.renames(true);
    patch.find_similar(Some(&mut find))?;
    let text = print_patch(&patch)?;
    if let Some(before) = before {
        assert_eq!(before, source_fingerprint(&work), "patch processing mutated retained source");
    }
    let mut status_opts = DiffOptions::new();
    status_opts.include_typechange(true).include_untracked(true).recurse_untracked_dirs(true);
    let mut staged = repo.diff_tree_to_index(head.as_ref(), Some(&index), Some(&mut status_opts))?;
    let mut status_find = DiffFindOptions::new();
    status_find.for_untracked(true);
    staged.find_similar(Some(&mut status_find))?;
    work.find_similar(Some(&mut status_find))?;
    Ok((text, pair(&staged, &work)))
}

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("repository path");
    let iterations: usize = args.next().map(|s| s.parse().unwrap()).unwrap_or(0);
    let mode = args.next();
    let force_index = mode.as_deref() == Some("force");
    let actual = actual_git::Repo::discover_at(Path::new(&path)).unwrap();
    let raw = Repository::open(&path).unwrap();
    if mode.as_deref() == Some("head-error") {
        let legacy_error = actual.working_tree_status().expect_err("fixture must fail the existing status reader");
        let error = snapshot(&raw, false, force_index).expect_err("damaged HEAD must not become an empty tree");
        actual.working_tree_snapshot().expect_err("implemented reader must reject damaged HEAD");
        println!("HEAD ERROR PASS code={:?} class={:?} legacy_error={legacy_error}", error.code(), error.class());
        return;
    }
    let expected_patch = actual.working_tree_diff().unwrap();
    let mut expected_status = actual.working_tree_status().unwrap();
    expected_status.sort_by(|a, b| a.path.cmp(&b.path));
    let (new_patch, new_status) = snapshot(&raw, true, force_index).unwrap();
    if expected_patch != new_patch {
        eprintln!("OLD PATCH:\n{expected_patch}\nNEW PATCH:\n{new_patch}");
        panic!("patch differs");
    }
    assert_eq!(expected_status, new_status, "status differs");
    let implemented = actual.working_tree_snapshot().unwrap();
    assert_eq!(implemented.patch, expected_patch, "implemented patch differs");
    let mut implemented_status = implemented.stages;
    implemented_status.sort_by(|a, b| a.path.cmp(&b.path));
    assert_eq!(implemented_status, expected_status, "implemented status differs");
    if new_status.len() <= 30 {
        println!("PASS libgit2={:?} force_index={force_index} statuses={new_status:?}", git2::Version::get().libgit2_version());
    } else {
        println!("PASS libgit2={:?} force_index={force_index} status_count={}", git2::Version::get().libgit2_version(), new_status.len());
    }
    if mode.as_deref() == Some("refresh") {
        let original_patch = new_patch;
        let index_path = raw.path().join("index");
        for command in [vec!["add", "a.txt"], vec!["reset", "-q", "HEAD", "--", "a.txt"]] {
            let result = std::process::Command::new("git").arg("-C").arg(&path).args(command).output().unwrap();
            assert!(result.status.success(), "external staging command failed");
            let before = std::fs::read(&index_path).unwrap();
            let before_mtime = std::fs::metadata(&index_path).unwrap().modified().unwrap();
            let (patch, stages) = snapshot(&raw, true, force_index).unwrap();
            assert_eq!(patch, original_patch, "staging-only command changed the net patch");
            let mut expected = actual.working_tree_status().unwrap();
            expected.sort_by(|a, b| a.path.cmp(&b.path));
            assert_eq!(stages, expected, "retained handle returned stale staging status");
            assert_eq!(std::fs::read(&index_path).unwrap(), before, "snapshot wrote index bytes");
            assert_eq!(std::fs::metadata(&index_path).unwrap().modified().unwrap(), before_mtime, "snapshot rewrote index");
            println!("REFRESH PASS stages={stages:?}");
        }
    }
    if iterations > 0 {
        let start = Instant::now();
        for _ in 0..iterations {
            std::hint::black_box(actual.working_tree_diff().unwrap());
            std::hint::black_box(actual.working_tree_status().unwrap());
        }
        let old_ms = start.elapsed().as_secs_f64() * 1000.0 / iterations as f64;
        let start = Instant::now();
        for _ in 0..iterations {
            std::hint::black_box(snapshot(&raw, false, force_index).unwrap());
        }
        let new_ms = start.elapsed().as_secs_f64() * 1000.0 / iterations as f64;
        println!("iterations={iterations} old_pair_ms={old_ms:.3} shared_ms={new_ms:.3} reduction_pct={:.2}", 100.0 * (old_ms - new_ms) / old_ms);
    }
}

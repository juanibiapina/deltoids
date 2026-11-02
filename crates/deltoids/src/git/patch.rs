//! Git patch text for a list of [`Change`]s, in the format
//! [`crate::parse::GitDiff::parse`] reads: `diff --git` headers, mode,
//! rename and full-length `index` lines, then `Binary files … differ` or
//! `---`/`+++` with unified hunks of three context lines.

use std::fmt::Write as _;

use gix::bstr::{BStr, ByteSlice};
use gix_imara_diff::{Algorithm, Diff, InternedInput};

use super::Error;
use super::diff::{Change, Side};

const CONTEXT: u32 = 3;
/// git judges a file binary by a NUL in its first 8000 bytes.
const BINARY_PROBE: usize = 8000;
/// git cuts a hunk header's function context at 80 bytes.
const FUNCNAME_MAX: usize = 80;
const NULL_ID: &str = "0000000000000000000000000000000000000000";

/// Write `changes` as patch text. `content` loads one side's bytes.
pub(super) fn write(
    changes: &[Change],
    mut content: impl FnMut(&Side) -> Result<Vec<u8>, Error>,
) -> Result<String, Error> {
    let mut out = String::new();
    for change in changes {
        let [old_path, new_path] = change.paths();
        let _ = writeln!(out, "diff --git a/{old_path} b/{new_path}");
        match (&change.old, &change.new) {
            (None, Some(new)) => {
                let data = content(new)?;
                let _ = writeln!(out, "new file mode {}", mode(new));
                let _ = writeln!(out, "index {NULL_ID}..{}", new.id);
                body(&mut out, None, Some((new.path.as_ref(), &data)));
            }
            (Some(old), None) => {
                let data = content(old)?;
                let _ = writeln!(out, "deleted file mode {}", mode(old));
                let _ = writeln!(out, "index {}..{NULL_ID}", old.id);
                body(&mut out, Some((old.path.as_ref(), &data)), None);
            }
            (Some(old), Some(new)) => {
                let before = content(old)?;
                let after = content(new)?;
                if old.mode != new.mode {
                    let _ = writeln!(out, "old mode {}", mode(old));
                    let _ = writeln!(out, "new mode {}", mode(new));
                }
                if change.is_rename() {
                    let similarity = similarity(old, new, &before, &after);
                    let _ = writeln!(out, "similarity index {similarity}%");
                    let _ = writeln!(out, "rename from {}", old.path);
                    let _ = writeln!(out, "rename to {}", new.path);
                }
                let typechange = change.is_typechange();
                if old.id == new.id && !typechange {
                    continue;
                }
                if old.mode == new.mode {
                    let _ = writeln!(out, "index {}..{} {}", old.id, new.id, mode(new));
                } else {
                    let _ = writeln!(out, "index {}..{}", old.id, new.id);
                }
                if typechange {
                    let _ = writeln!(out, "--- a/{}\n+++ b/{}", old.path, new.path);
                    continue;
                }
                body(
                    &mut out,
                    Some((old.path.as_ref(), &before)),
                    Some((new.path.as_ref(), &after)),
                );
            }
            (None, None) => {}
        }
    }
    Ok(out)
}

fn mode(side: &Side) -> String {
    format!("{:06o}", side.mode.bits())
}

fn body(out: &mut String, old: Option<(&BStr, &[u8])>, new: Option<(&BStr, &[u8])>) {
    let label = |side: Option<(&BStr, &[u8])>, prefix: &str| match side {
        Some((path, _)) => format!("{prefix}/{path}"),
        None => "/dev/null".to_string(),
    };
    let (a, b) = (label(old, "a"), label(new, "b"));
    if [old, new].iter().flatten().any(|(_, data)| is_binary(data)) {
        let _ = writeln!(out, "Binary files {a} and {b} differ");
        return;
    }
    let _ = writeln!(out, "--- {a}\n+++ {b}");
    hunks(
        out,
        old.map(|(_, d)| d).unwrap_or_default(),
        new.map(|(_, d)| d).unwrap_or_default(),
    );
}

fn is_binary(data: &[u8]) -> bool {
    data[..data.len().min(BINARY_PROBE)].contains(&0)
}

/// Percentage of `old`'s bytes that survive into `new`, measured on the
/// larger side, the way git reports a rename's similarity index.
fn similarity(old: &Side, new: &Side, before: &[u8], after: &[u8]) -> u64 {
    if old.id == new.id {
        return 100;
    }
    let larger = before.len().max(after.len()) as u64;
    if larger == 0 {
        return 100;
    }
    let input = InternedInput::new(before, after);
    let diff = Diff::compute(Algorithm::Myers, &input);
    let kept: u64 = input
        .before
        .iter()
        .enumerate()
        .filter(|(i, _)| !diff.is_removed(*i as u32))
        .map(|(_, token)| input.interner[*token].len() as u64)
        .sum();
    kept * 100 / larger
}

fn hunks(out: &mut String, before: &[u8], after: &[u8]) {
    let input = InternedInput::new(before, after);
    let mut diff = Diff::compute(Algorithm::Myers, &input);
    diff.postprocess_no_heuristic(&input);
    let old_len = input.before.len() as u32;
    let new_len = input.after.len() as u32;

    let mut groups: Vec<(std::ops::Range<u32>, std::ops::Range<u32>)> = Vec::new();
    for hunk in diff.hunks() {
        match groups.last_mut() {
            Some((old, new)) if hunk.before.start - old.end <= 2 * CONTEXT => {
                old.end = hunk.before.end;
                new.end = hunk.after.end;
            }
            _ => groups.push((hunk.before.clone(), hunk.after.clone())),
        }
    }

    let line =
        |side: &[gix_imara_diff::Token], i: u32| -> &[u8] { input.interner[side[i as usize]] };
    for (old, new) in groups {
        let lead = old.start.min(CONTEXT);
        let old_start = old.start - lead;
        let new_start = new.start - lead;
        let old_end = (old.end + CONTEXT).min(old_len);
        let new_end = (new.end + (old_end - old.end)).min(new_len);
        let _ = write!(
            out,
            "@@ -{} +{} @@",
            range(old_start, old_end - old_start),
            range(new_start, new_end - new_start)
        );
        if let Some(name) = funcname(&input.before, &input.interner, old_start) {
            let _ = write!(out, " {name}");
        }
        out.push('\n');

        let (mut o, mut n) = (old_start, new_start);
        while o < old_end || n < new_end {
            let removed = o < old_end && diff.is_removed(o);
            let added = n < new_end && diff.is_added(n);
            if removed {
                emit(out, '-', line(&input.before, o));
                o += 1;
            } else if added {
                emit(out, '+', line(&input.after, n));
                n += 1;
            } else {
                emit(out, ' ', line(&input.before, o));
                o += 1;
                n += 1;
            }
        }
    }
}

/// A hunk header range: `start,count`, with git's conventions of a
/// 1-based start, the line before for an empty range, and no count of 1.
fn range(start: u32, count: u32) -> String {
    match count {
        0 => format!("{start},0"),
        1 => format!("{}", start + 1),
        _ => format!("{},{count}", start + 1),
    }
}

fn emit(out: &mut String, origin: char, line: &[u8]) {
    out.push(origin);
    out.push_str(&String::from_utf8_lossy(line));
    if !line.ends_with(b"\n") {
        out.push_str("\n\\ No newline at end of file\n");
    }
}

/// The function context git's default rule picks for a hunk starting at
/// old line `start`: the closest earlier line that begins with a letter,
/// `_`, or `$`, without its line ending, cut to 80 bytes.
fn funcname<'a>(
    before: &[gix_imara_diff::Token],
    interner: &'a gix_imara_diff::Interner<&'a [u8]>,
    start: u32,
) -> Option<String> {
    before[..start as usize].iter().rev().find_map(|token| {
        let line: &[u8] = interner[*token];
        let first = *line.first()?;
        if !(first.is_ascii_alphabetic() || first == b'_' || first == b'$') {
            return None;
        }
        let line = line.trim_end();
        let cut = &line[..line.len().min(FUNCNAME_MAX)];
        Some(cut.to_str_lossy().trim_end().to_string())
    })
}

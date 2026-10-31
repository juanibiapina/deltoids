//! Tree-building axis: the sidebar's rows, built by the shared
//! [`deltoids_review::tree`].

use deltoids_review::tree::{self, TreeRow};

use super::Root;
use super::status::{SidebarFile, display_path};

/// One renderable row in the sidebar. `Dir` rows are headers; `File`
/// rows are leaves and are the only rows the user can land on.
#[derive(Debug, Clone)]
pub(super) enum Row {
    /// The commit or branch under review, above the tree.
    Root(Root),
    /// The rule between the root row and the tree. Never selected.
    Rule,
    Dir {
        /// Display label (with trailing `/`, possibly multi-segment for
        /// collapsed chains like `deltoids/src/`).
        label: String,
        /// Indent depth — number of parent directories above this one.
        depth: usize,
    },
    File {
        /// Index into the original `&[SidebarFile]` slice.
        file_index: usize,
        /// File's leaf name (`lib.rs`).
        name: String,
        /// Indent depth — number of parent directories above this file.
        depth: usize,
    },
}

/// Build the sidebar rows for `files` with [`deltoids_review::tree::rows`].
/// Files whose `hidden[file_index]` is `true` are left out, and so are
/// directories left without files; row file indices stay those of `files`.
pub(super) fn build_rows(files: &[SidebarFile<'_>], hidden: &[bool]) -> Vec<Row> {
    let shown = files
        .iter()
        .enumerate()
        .filter(|(index, _)| !hidden.get(*index).copied().unwrap_or(false))
        .map(|(index, file)| (index, display_path(file.file)));
    tree::rows(shown)
        .into_iter()
        .map(|row| match row {
            TreeRow::Dir { label, depth } => Row::Dir { label, depth },
            TreeRow::File { index, name, depth } => Row::File {
                file_index: index,
                name,
                depth,
            },
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sidebar::test_support::*;

    #[test]
    fn build_rows_groups_files_under_common_directory() {
        let a = fd("src/a.rs");
        let b = fd("src/b.rs");
        let files = vec![
            SidebarFile {
                file: &a,
                added: 0,
                deleted: 0,
                stage: None,
            },
            SidebarFile {
                file: &b,
                added: 0,
                deleted: 0,
                stage: None,
            },
        ];
        let rows = build_rows(&files, &[]);
        // 1 dir header + 2 files
        assert_eq!(rows.len(), 3);
        match &rows[0] {
            Row::Dir { label, depth } => {
                assert_eq!(label, "src/");
                assert_eq!(*depth, 0);
            }
            other => panic!("expected dir header, got {other:?}"),
        }
        match &rows[1] {
            Row::File { name, depth, .. } => {
                assert_eq!(name, "a.rs");
                assert_eq!(*depth, 1);
            }
            other => panic!("expected file row, got {other:?}"),
        }
    }

    #[test]
    fn build_rows_collapses_single_child_directory_chain() {
        // crates/deltoids/src/{lib.rs,parse.rs} — `crates/` has one
        // subdir `deltoids/`, which has one subdir `src/` → all three
        // collapse into a single header `crates/deltoids/src/`.
        let a = fd("crates/deltoids/src/lib.rs");
        let b = fd("crates/deltoids/src/parse.rs");
        let files = vec![
            SidebarFile {
                file: &a,
                added: 0,
                deleted: 0,
                stage: None,
            },
            SidebarFile {
                file: &b,
                added: 0,
                deleted: 0,
                stage: None,
            },
        ];
        let rows = build_rows(&files, &[]);
        assert_eq!(rows.len(), 3);
        match &rows[0] {
            Row::Dir { label, depth } => {
                assert_eq!(label, "crates/deltoids/src/");
                assert_eq!(*depth, 0);
            }
            other => panic!("expected collapsed dir header, got {other:?}"),
        }
    }

    #[test]
    fn build_rows_does_not_collapse_when_dir_has_multiple_children() {
        // crates/{deltoids/src/lib.rs, deltoids-cli/src/lib.rs} —
        // `crates/` has two children, must stay on its own row.
        let a = fd("crates/deltoids/src/lib.rs");
        let b = fd("crates/deltoids-cli/src/lib.rs");
        let files = vec![
            SidebarFile {
                file: &a,
                added: 0,
                deleted: 0,
                stage: None,
            },
            SidebarFile {
                file: &b,
                added: 0,
                deleted: 0,
                stage: None,
            },
        ];
        let rows = build_rows(&files, &[]);
        // crates/ + deltoids/src/ + lib.rs + deltoids-cli/src/ + lib.rs
        assert_eq!(rows.len(), 5);
        match &rows[0] {
            Row::Dir { label, .. } => assert_eq!(label, "crates/"),
            other => panic!("expected crates/, got {other:?}"),
        }
    }

    #[test]
    fn build_rows_handles_top_level_files() {
        let a = fd("README.md");
        let b = fd("Cargo.toml");
        let files = vec![
            SidebarFile {
                file: &a,
                added: 0,
                deleted: 0,
                stage: None,
            },
            SidebarFile {
                file: &b,
                added: 0,
                deleted: 0,
                stage: None,
            },
        ];
        let rows = build_rows(&files, &[]);
        assert_eq!(rows.len(), 2);
        for row in &rows {
            match row {
                Row::File { depth, .. } => assert_eq!(*depth, 0),
                other => panic!("expected file at top level, got {other:?}"),
            }
        }
    }

    #[test]
    fn build_rows_sorts_mixed_by_name() {
        let a = fd("zzz.rs");
        let b = fd("src/a.rs");
        let c = fd("aaa.rs");
        let files = vec![
            SidebarFile {
                file: &a,
                added: 0,
                deleted: 0,
                stage: None,
            },
            SidebarFile {
                file: &b,
                added: 0,
                deleted: 0,
                stage: None,
            },
            SidebarFile {
                file: &c,
                added: 0,
                deleted: 0,
                stage: None,
            },
        ];
        let rows = build_rows(&files, &[]);
        // Expect (mixed): aaa.rs ; src/ ; src/a.rs ; zzz.rs
        match &rows[0] {
            Row::File { name, .. } => assert_eq!(name, "aaa.rs"),
            other => panic!("expected aaa.rs first, got {other:?}"),
        }
        match &rows[1] {
            Row::Dir { label, .. } => assert_eq!(label, "src/"),
            other => panic!("expected src/ second, got {other:?}"),
        }
        match &rows[2] {
            Row::File { name, .. } => assert_eq!(name, "a.rs"),
            other => panic!("expected src/a.rs third, got {other:?}"),
        }
        match &rows[3] {
            Row::File { name, .. } => assert_eq!(name, "zzz.rs"),
            other => panic!("expected zzz.rs last, got {other:?}"),
        }
    }
}

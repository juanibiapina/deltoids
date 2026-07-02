//! Tree-building axis: turn the flat file list into a sorted, collapsed
//! path tree and walk it into renderable [`Row`]s.

use super::status::{SidebarFile, display_path};

/// One renderable row in the sidebar. `Dir` rows are headers; `File`
/// rows are leaves and are the only rows the user can land on.
#[derive(Debug, Clone)]
pub(super) enum Row {
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

/// Internal tree node used during construction. Collapsed into [`Row`]s
/// during a depth-first walk.
#[derive(Debug)]
struct Node {
    /// Directory segment (or file name for leaves). Empty at the root.
    name: String,
    /// `Some(file_index)` when this node is a leaf; `None` for directories.
    file_index: Option<usize>,
    /// Children sorted by name (directories and files interleaved).
    children: Vec<Node>,
}

impl Node {
    fn new_dir(name: String) -> Self {
        Self {
            name,
            file_index: None,
            children: Vec::new(),
        }
    }
}

/// Build a tree from the file paths, then walk it to produce [`Row`]s.
///
/// Tree building rule (matches lazygit's default `mixed` order):
///
/// 1. Split each file's display path on `/`; insert the leaf as a child
///    of the deepest directory.
/// 2. Sort each directory's children alphabetically by name, with
///    directories and files interleaved (no dir-vs-file tiebreak).
/// 3. Collapse single-child directory chains: if a directory has
///    exactly one child and that child is a directory, fold the child's
///    name into the parent's label and continue collapsing.
pub(super) fn build_rows(files: &[SidebarFile<'_>]) -> Vec<Row> {
    let mut root = Node::new_dir(String::new());

    for (file_index, file) in files.iter().enumerate() {
        let path = display_path(file.file);
        insert_path(&mut root, path, file_index);
    }

    sort_tree(&mut root);

    let mut rows = Vec::new();
    walk(&root, 0, &mut rows);
    rows
}

fn insert_path(root: &mut Node, path: &str, file_index: usize) {
    let mut current = root;
    let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    let Some((leaf, dirs)) = segments.split_last() else {
        return;
    };
    for segment in dirs {
        let position = current
            .children
            .iter()
            .position(|child| child.file_index.is_none() && child.name == *segment);
        let index = match position {
            Some(idx) => idx,
            None => {
                current.children.push(Node::new_dir(segment.to_string()));
                current.children.len() - 1
            }
        };
        current = &mut current.children[index];
    }
    current.children.push(Node {
        name: leaf.to_string(),
        file_index: Some(file_index),
        children: Vec::new(),
    });
}

fn sort_tree(node: &mut Node) {
    node.children.sort_by(|a, b| a.name.cmp(&b.name));
    for child in &mut node.children {
        sort_tree(child);
    }
}

fn walk(node: &Node, depth: usize, out: &mut Vec<Row>) {
    // Root node: emit children directly (no header for the empty root).
    if node.name.is_empty() && node.file_index.is_none() {
        for child in &node.children {
            walk(child, depth, out);
        }
        return;
    }

    if let Some(file_index) = node.file_index {
        out.push(Row::File {
            file_index,
            name: node.name.clone(),
            depth,
        });
        return;
    }

    // Directory: collapse single-child directory chains into a combined label.
    let mut label = format!("{}/", node.name);
    let mut current = node;
    while current.children.len() == 1 && current.children[0].file_index.is_none() {
        current = &current.children[0];
        label.push_str(&current.name);
        label.push('/');
    }
    out.push(Row::Dir { label, depth });
    for child in &current.children {
        walk(child, depth + 1, out);
    }
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
        let rows = build_rows(&files);
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
        let rows = build_rows(&files);
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
        let rows = build_rows(&files);
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
        let rows = build_rows(&files);
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
        let rows = build_rows(&files);
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

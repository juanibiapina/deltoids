//! The file tree of a review: group changed paths by directory, sort them,
//! and fold chains of single-child directories into one row.

/// One row of the file tree, in display order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TreeRow {
    Dir {
        /// Folded path segments with a trailing `/` (`crates/deltoids/src/`).
        label: String,
        /// Number of directory rows above this one.
        depth: usize,
    },
    File {
        /// The index the caller gave with the path.
        index: usize,
        /// The file's last path segment (`lib.rs`).
        name: String,
        /// Number of directory rows above this file.
        depth: usize,
    },
}

#[derive(Debug)]
struct Node {
    name: String,
    index: Option<usize>,
    children: Vec<Node>,
}

impl Node {
    fn dir(name: String) -> Self {
        Self {
            name,
            index: None,
            children: Vec::new(),
        }
    }
}

/// Build the tree rows for `(index, path)` pairs.
///
/// 1. Split each path on `/` and insert the file under its deepest
///    directory.
/// 2. Sort each directory's children by name with byte order, directories
///    and files interleaved (lazygit's `mixed` order).
/// 3. Fold a directory whose only child is a directory into one row.
///
/// Paths with no segments are skipped. Indices pass through unchanged, so a
/// caller that leaves files out keeps its own numbering.
pub fn rows<'a>(files: impl IntoIterator<Item = (usize, &'a str)>) -> Vec<TreeRow> {
    let mut root = Node::dir(String::new());
    for (index, path) in files {
        insert(&mut root, path, index);
    }
    sort(&mut root);
    let mut out = Vec::new();
    for child in &root.children {
        walk(child, 0, &mut out);
    }
    out
}

fn insert(root: &mut Node, path: &str, index: usize) {
    let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    let Some((leaf, dirs)) = segments.split_last() else {
        return;
    };
    let mut current = root;
    for segment in dirs {
        let position = current
            .children
            .iter()
            .position(|child| child.index.is_none() && child.name == *segment);
        let at = position.unwrap_or_else(|| {
            current.children.push(Node::dir((*segment).to_string()));
            current.children.len() - 1
        });
        current = &mut current.children[at];
    }
    current.children.push(Node {
        name: (*leaf).to_string(),
        index: Some(index),
        children: Vec::new(),
    });
}

fn sort(node: &mut Node) {
    node.children.sort_by(|a, b| a.name.cmp(&b.name));
    for child in &mut node.children {
        sort(child);
    }
}

fn walk(node: &Node, depth: usize, out: &mut Vec<TreeRow>) {
    if let Some(index) = node.index {
        out.push(TreeRow::File {
            index,
            name: node.name.clone(),
            depth,
        });
        return;
    }
    let mut label = format!("{}/", node.name);
    let mut current = node;
    while let [only] = current.children.as_slice() {
        if only.index.is_some() {
            break;
        }
        current = only;
        label.push_str(&current.name);
        label.push('/');
    }
    out.push(TreeRow::Dir { label, depth });
    for child in &current.children {
        walk(child, depth + 1, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(paths: &[&str]) -> Vec<TreeRow> {
        rows(paths.iter().copied().enumerate())
    }

    fn dir(label: &str, depth: usize) -> TreeRow {
        TreeRow::Dir {
            label: label.to_string(),
            depth,
        }
    }

    fn file(index: usize, name: &str, depth: usize) -> TreeRow {
        TreeRow::File {
            index,
            name: name.to_string(),
            depth,
        }
    }

    #[test]
    fn groups_files_under_their_directory() {
        assert_eq!(
            tree(&["src/a.rs", "src/b.rs"]),
            vec![dir("src/", 0), file(0, "a.rs", 1), file(1, "b.rs", 1)]
        );
    }

    #[test]
    fn folds_a_chain_of_single_child_directories() {
        assert_eq!(
            tree(&["crates/deltoids/src/lib.rs", "crates/deltoids/src/parse.rs"]),
            vec![
                dir("crates/deltoids/src/", 0),
                file(0, "lib.rs", 1),
                file(1, "parse.rs", 1),
            ]
        );
    }

    #[test]
    fn keeps_a_directory_with_several_children_on_its_own_row() {
        assert_eq!(
            tree(&[
                "crates/deltoids/src/lib.rs",
                "crates/deltoids-cli/src/lib.rs"
            ]),
            vec![
                dir("crates/", 0),
                dir("deltoids/src/", 1),
                file(0, "lib.rs", 2),
                dir("deltoids-cli/src/", 1),
                file(1, "lib.rs", 2),
            ]
        );
    }

    #[test]
    fn keeps_top_level_files_at_depth_zero() {
        assert_eq!(
            tree(&["README.md", "Cargo.toml"]),
            vec![file(1, "Cargo.toml", 0), file(0, "README.md", 0)]
        );
    }

    #[test]
    fn interleaves_directories_and_files_by_name() {
        assert_eq!(
            tree(&["zzz.rs", "src/a.rs", "aaa.rs"]),
            vec![
                file(2, "aaa.rs", 0),
                dir("src/", 0),
                file(1, "a.rs", 1),
                file(0, "zzz.rs", 0),
            ]
        );
    }

    #[test]
    fn sorts_by_bytes_so_uppercase_comes_first() {
        assert_eq!(
            tree(&["b.rs", "B.rs", "a.rs"]),
            vec![file(1, "B.rs", 0), file(2, "a.rs", 0), file(0, "b.rs", 0)]
        );
    }

    #[test]
    fn a_directory_holding_a_file_and_a_directory_does_not_fold() {
        assert_eq!(
            tree(&["x/y/z.rs", "x/w.rs"]),
            vec![
                dir("x/", 0),
                file(1, "w.rs", 1),
                dir("y/", 1),
                file(0, "z.rs", 2),
            ]
        );
    }

    #[test]
    fn keeps_caller_indices_and_skips_empty_paths() {
        assert_eq!(
            rows([(7, "src/a.rs"), (9, ""), (3, "b.rs")]),
            vec![file(3, "b.rs", 0), dir("src/", 0), file(7, "a.rs", 1)]
        );
    }
}

//! Lazygit-inspired left sidebar for `rv`: a scrollable file tree with
//! status badges, nerd-font icons, and per-file line-delta counts.
//!
//! Public entry points:
//!
//! - [`Sidebar::build`] — construct from a slice of [`SidebarFile`]s.
//! - [`Sidebar::rows`] — pre-styled rows ready to render in a
//!   [`ratatui::widgets::Paragraph`].
//! - [`Sidebar::selected_file_index`] — index into the original file
//!   slice, or `None` when a directory row is selected.
//! - [`Sidebar::move_up`] / [`Sidebar::move_down`] / [`Sidebar::top`] /
//!   [`Sidebar::bottom`] / [`Sidebar::page_up`] / [`Sidebar::page_down`]
//!   — navigation. Selection skips directory rows so j/k always lands on
//!   a file.
//! - [`Sidebar::scroll`] — current scroll offset (auto-tracked to keep
//!   selection visible).
//! - [`Sidebar::row_count`] — total renderable rows.
//!
//! Implementation notes:
//!
//! - Tree building uses lazygit-style "single-child directory chain
//!   collapsing": a directory whose only child is another directory is
//!   merged into the child's name (`crates/deltoids/src/`).
//! - Icons come from a small built-in extension table; users without a
//!   nerd font can opt out via `RV_NO_ICONS=1`.
//! - Status detection reads `FileDiff` paths and index hashes (null hash
//!   → side absent → added/deleted; otherwise modified or renamed).

use std::ops::Range;

use deltoids::Theme;
use deltoids::parse::FileDiff;
use deltoids::render_tui::rgb_to_color;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

/// One file's worth of input the sidebar needs to render.
///
/// A thin view over the data the rv main loop already has: the parsed
/// [`FileDiff`] (for paths, status, rename info) plus a count of added
/// and deleted lines (computed once from the resolved [`Diff`]).
#[derive(Debug, Clone)]
pub struct SidebarFile<'a> {
    pub file: &'a FileDiff,
    pub added: usize,
    pub deleted: usize,
}

/// Whether the sidebar treats the file as added, deleted, modified, or
/// renamed. Drives the colored single-letter status badge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileStatus {
    Added,
    Deleted,
    Modified,
    Renamed,
}

impl FileStatus {
    /// Single-letter badge displayed at the start of each file row.
    pub fn badge(self) -> char {
        match self {
            FileStatus::Added => 'A',
            FileStatus::Deleted => 'D',
            FileStatus::Modified => 'M',
            FileStatus::Renamed => 'R',
        }
    }
}

/// Classify a [`FileDiff`] as added / deleted / modified / renamed.
///
/// Added: old path is `/dev/null` or old hash is the null oid.
/// Deleted: new path is `/dev/null` or new hash is the null oid.
/// Renamed: `rename_from` is set and the file is neither added nor
/// deleted.
/// Modified: everything else.
pub fn file_status(file: &FileDiff) -> FileStatus {
    let old_absent = file.old_path == "/dev/null" || is_null_hash(file.old_hash.as_deref());
    let new_absent = file.new_path == "/dev/null" || is_null_hash(file.new_hash.as_deref());

    match (old_absent, new_absent) {
        (true, false) => FileStatus::Added,
        (false, true) => FileStatus::Deleted,
        _ if file.rename_from.is_some() => FileStatus::Renamed,
        _ => FileStatus::Modified,
    }
}

fn is_null_hash(hash: Option<&str>) -> bool {
    hash.is_some_and(|h| h.chars().all(|c| c == '0'))
}

/// Effective path for display: prefer `new_path`, fall back to
/// `old_path` for deletions (`new_path == "/dev/null"`).
pub fn display_path(file: &FileDiff) -> &str {
    if file.new_path == "/dev/null" {
        &file.old_path
    } else {
        &file.new_path
    }
}

// ---------------------------------------------------------------------------
// Tree building
// ---------------------------------------------------------------------------

/// One renderable row in the sidebar. `Dir` rows are headers; `File`
/// rows are leaves and are the only rows the user can land on.
#[derive(Debug, Clone)]
enum Row {
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
    /// Children sorted by name with directories first.
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
/// Tree building rule (lazygit-style):
///
/// 1. Split each file's display path on `/`; insert the leaf as a child
///    of the deepest directory.
/// 2. Sort each directory's children: directories before files,
///    alphabetic within each group.
/// 3. Collapse single-child directory chains: if a directory has
///    exactly one child and that child is a directory, fold the child's
///    name into the parent's label and continue collapsing.
fn build_rows(files: &[SidebarFile<'_>]) -> Vec<Row> {
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
    node.children.sort_by(|a, b| {
        match (a.file_index.is_none(), b.file_index.is_none()) {
            // Both dirs or both files: alphabetic.
            (true, true) | (false, false) => a.name.cmp(&b.name),
            // Dir before file.
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
        }
    });
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

// ---------------------------------------------------------------------------
// Icon lookup
// ---------------------------------------------------------------------------

/// Whether icons were requested at build time (controlled by the
/// `RV_NO_ICONS` environment variable). Captured per-`Sidebar` so tests
/// can override without touching the global env.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconMode {
    On,
    Off,
}

impl IconMode {
    /// Read `RV_NO_ICONS` from the environment. Set to anything → off.
    pub fn from_env() -> Self {
        match std::env::var_os("RV_NO_ICONS") {
            Some(v) if !v.is_empty() => IconMode::Off,
            _ => IconMode::On,
        }
    }
}

/// Pick a nerd-font glyph for the given filename. Looks at exact
/// filenames first (Cargo.toml, README.md, Dockerfile, …) then falls
/// back to extensions, then to a generic file glyph.
fn file_icon(name: &str) -> &'static str {
    // Exact filename matches (project-level files that deserve their own glyph).
    let lower = name.to_ascii_lowercase();
    let exact: &[(&str, &str)] = &[
        ("cargo.toml", "\u{e7a8}"), // rust crate gear
        ("cargo.lock", "\u{e7a8}"),
        ("readme.md", "\u{f48a}"), // markdown
        ("license", "\u{f0fc3}"),
        ("license.md", "\u{f0fc3}"),
        ("dockerfile", "\u{f308}"),
        (".gitignore", "\u{f1d3}"),
        (".gitattributes", "\u{f1d3}"),
        ("makefile", "\u{e779}"),
        ("package.json", "\u{e718}"),
        ("package-lock.json", "\u{e718}"),
    ];
    for (k, v) in exact {
        if lower == *k {
            return v;
        }
    }

    let ext = name.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
    match ext.to_ascii_lowercase().as_str() {
        "rs" => "\u{e7a8}", // rust
        "toml" => "\u{e6b2}",
        "md" | "markdown" => "\u{f48a}",
        "json" => "\u{e60b}",
        "yaml" | "yml" => "\u{f481}",
        "py" => "\u{e606}",
        "js" | "mjs" | "cjs" => "\u{e74e}",
        "ts" | "tsx" => "\u{e628}",
        "jsx" => "\u{e7ba}",
        "go" => "\u{e65e}",
        "html" | "htm" => "\u{f13b}",
        "css" => "\u{e749}",
        "scss" | "sass" => "\u{e74b}",
        "sh" | "bash" | "zsh" | "fish" => "\u{f489}",
        "c" => "\u{e61e}",
        "h" => "\u{f0fd1}",
        "cpp" | "cxx" | "cc" => "\u{e61d}",
        "hpp" | "hxx" => "\u{f0fd1}",
        "java" => "\u{e738}",
        "rb" => "\u{e739}",
        "php" => "\u{e73d}",
        "lua" => "\u{e620}",
        "vim" => "\u{e7c5}",
        "txt" | "log" => "\u{f0219}",
        "lock" => "\u{f023}",
        "xml" => "\u{f72d}",
        "svg" => "\u{f81f}",
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "ico" => "\u{f1c5}",
        "zip" | "tar" | "gz" | "xz" | "bz2" | "7z" => "\u{f1c6}",
        _ => "\u{f15b}", // generic file
    }
}

/// Folder glyph used for directory rows. Future work could swap to a
/// closed-folder glyph when collapse/expand state lands; for now we
/// always render it as open since the tree is always fully expanded.
const ICON_DIR_OPEN: &str = "\u{f07c}";

// ---------------------------------------------------------------------------
// Sidebar state
// ---------------------------------------------------------------------------

/// Aggregate statistics about the resolved file set, summarised for
/// display below the sidebar (total file count plus cumulative line
/// deltas across all files).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Totals {
    pub files: usize,
    pub added: usize,
    pub deleted: usize,
}

/// Pre-rendered rows + selection + scroll. Driven by the rv main loop:
/// build once per resolved file set, then call `move_*` and `rows()`
/// each frame.
#[derive(Debug)]
pub struct Sidebar {
    rows: Vec<Row>,
    /// Index of currently-highlighted row (always a `Row::File` if any
    /// files exist).
    selected: usize,
    /// First visible row in the viewport.
    scroll: usize,
    /// Cached rendered lines, regenerated whenever `selected` or the
    /// theme changes.
    rendered: Vec<Line<'static>>,
    /// Captured at build time so tests can supply specific values.
    icons: IconMode,
    /// Cached for re-rendering after selection moves.
    /// Stored as `(label, status, deltas, rename_arrow)` per row.
    file_meta: Vec<FileRowMeta>,
    /// Theme stored so re-rendering on selection change is self-contained.
    theme: Theme,
    /// Aggregate statistics computed at build time.
    totals: Totals,
}

#[derive(Debug, Clone)]
struct FileRowMeta {
    /// `None` for directory rows.
    status: Option<FileStatus>,
    /// `None` for directory rows.
    deltas: Option<(usize, usize)>,
    /// `Some((old, new))` for renamed files; the row label shows
    /// `old → new` instead of just `new`.
    rename: Option<(String, String)>,
}

impl Sidebar {
    /// Build the sidebar from a slice of files plus a theme. Captures the
    /// icon mode from the environment.
    pub fn build(files: &[SidebarFile<'_>], theme: &Theme) -> Self {
        Self::build_with_icons(files, theme, IconMode::from_env())
    }

    /// Same as `build` but with an explicit icon mode (for tests).
    pub fn build_with_icons(files: &[SidebarFile<'_>], theme: &Theme, icons: IconMode) -> Self {
        let rows = build_rows(files);
        let file_meta = rows
            .iter()
            .map(|row| match row {
                Row::Dir { .. } => FileRowMeta {
                    status: None,
                    deltas: None,
                    rename: None,
                },
                Row::File { file_index, .. } => {
                    let f = &files[*file_index];
                    let status = file_status(f.file);
                    let rename = if status == FileStatus::Renamed {
                        f.file
                            .rename_from
                            .as_ref()
                            .map(|old| (rename_leaf(old), rename_leaf(&f.file.new_path)))
                    } else {
                        None
                    };
                    FileRowMeta {
                        status: Some(status),
                        deltas: Some((f.added, f.deleted)),
                        rename,
                    }
                }
            })
            .collect::<Vec<_>>();

        let selected = rows
            .iter()
            .position(|row| matches!(row, Row::File { .. }))
            .unwrap_or(0);

        let totals = Totals {
            files: files.len(),
            added: files.iter().map(|f| f.added).sum(),
            deleted: files.iter().map(|f| f.deleted).sum(),
        };

        let mut sidebar = Self {
            rows,
            selected,
            scroll: 0,
            rendered: Vec::new(),
            icons,
            file_meta,
            theme: theme.clone(),
            totals,
        };
        sidebar.render_all();
        sidebar
    }

    /// File count + aggregate `+`/`-` totals, computed at build time.
    pub fn totals(&self) -> Totals {
        self.totals
    }

    /// Total number of renderable rows (dirs + files).
    pub fn row_count(&self) -> usize {
        self.rows.len()
    }

    /// File indices (into the original `&[SidebarFile]` slice) in the
    /// order they appear in the sidebar.
    ///
    /// Used by the rv main loop to render diff content in tree order so
    /// the diff pane's vertical layout always matches the sidebar.
    pub fn display_order(&self) -> Vec<usize> {
        self.rows
            .iter()
            .filter_map(|r| match r {
                Row::File { file_index, .. } => Some(*file_index),
                _ => None,
            })
            .collect()
    }

    /// Pre-styled rows ready for a `Paragraph`. Borrowed; owned by the
    /// sidebar.
    pub fn rows(&self) -> &[Line<'static>] {
        &self.rendered
    }

    /// Currently-selected row index.
    pub fn selected(&self) -> usize {
        self.selected
    }

    /// Index into the original `&[SidebarFile]` slice for the selected
    /// row, or `None` when a directory row is selected.
    ///
    /// Production code uses [`Sidebar::nearest_file_index`] instead, so
    /// the diff pane stays in sync as the user traverses dirs; this
    /// stricter accessor is kept for tests and for callers that want to
    /// distinguish "a file is selected" from "a directory is selected".
    #[allow(dead_code)]
    pub fn selected_file_index(&self) -> Option<usize> {
        match self.rows.get(self.selected) {
            Some(Row::File { file_index, .. }) => Some(*file_index),
            _ => None,
        }
    }

    /// First row visible in the viewport. Auto-tracked by the move
    /// methods so the selection always stays in view.
    pub fn scroll(&self) -> usize {
        self.scroll
    }

    /// Whether the currently-selected row is a directory header.
    pub fn selected_is_dir(&self) -> bool {
        matches!(self.rows.get(self.selected), Some(Row::Dir { .. }))
    }

    /// Input index of the file the diff pane should snap to for the
    /// current selection.
    ///
    /// On a file row this is just the selected file. On a directory
    /// row it's the first file inside that directory's subtree (the
    /// next file row at or after the selection — since each directory
    /// header is followed by its contents). `None` only when there are
    /// no files at all.
    pub fn nearest_file_index(&self) -> Option<usize> {
        self.rows.iter().skip(self.selected).find_map(|r| match r {
            Row::File { file_index, .. } => Some(*file_index),
            Row::Dir { .. } => None,
        })
    }

    /// Display-order positions of every file under the selected
    /// directory's subtree, as a half-open range.
    ///
    /// Returns `None` when a file row is selected (no filtering
    /// applies). When a directory is selected, the returned range
    /// always covers a contiguous run of files — the renderer can
    /// slice the diff lines between the first file's offset and the
    /// last file's end without skipping anything.
    pub fn subtree_display_range(&self) -> Option<Range<usize>> {
        let Some(Row::Dir {
            depth: parent_depth,
            ..
        }) = self.rows.get(self.selected)
        else {
            return None;
        };
        let parent_depth = *parent_depth;

        // Files preceding the selected dir count as "before" the range.
        let start = self.rows[..self.selected]
            .iter()
            .filter(|r| matches!(r, Row::File { .. }))
            .count();

        // Walk forward until we exit the subtree (any row at depth
        // <= parent_depth ends it). Count files inside.
        let mut count = 0usize;
        for row in &self.rows[self.selected + 1..] {
            let row_depth = match row {
                Row::Dir { depth, .. } => *depth,
                Row::File { depth, .. } => *depth,
            };
            if row_depth <= parent_depth {
                break;
            }
            if matches!(row, Row::File { .. }) {
                count += 1;
            }
        }

        if count == 0 {
            return None;
        }
        Some(start..start + count)
    }

    /// Move to the next row (file or directory).
    pub fn move_down(&mut self, viewport: usize) {
        if self.selected + 1 < self.rows.len() {
            self.set_selected(self.selected + 1, viewport);
        }
    }

    /// Move to the previous row (file or directory).
    pub fn move_up(&mut self, viewport: usize) {
        if self.selected > 0 {
            self.set_selected(self.selected - 1, viewport);
        }
    }

    /// Jump to the first row.
    pub fn top(&mut self, viewport: usize) {
        if !self.rows.is_empty() {
            self.set_selected(0, viewport);
        }
    }

    /// Jump to the last row.
    pub fn bottom(&mut self, viewport: usize) {
        if let Some(last) = self.rows.len().checked_sub(1) {
            self.set_selected(last, viewport);
        }
    }

    /// Move down by `viewport` rows, clamped at the last row.
    pub fn page_down(&mut self, viewport: usize) {
        let target = self
            .selected
            .saturating_add(viewport.max(1))
            .min(self.rows.len().saturating_sub(1));
        self.set_selected(target, viewport);
    }

    /// Move up by `viewport` rows, clamped at the first row.
    pub fn page_up(&mut self, viewport: usize) {
        let target = self.selected.saturating_sub(viewport.max(1));
        self.set_selected(target, viewport);
    }

    fn set_selected(&mut self, target: usize, viewport: usize) {
        if target == self.selected {
            return;
        }
        self.selected = target;
        self.adjust_scroll(viewport);
        self.render_all();
    }

    /// Keep the selected row inside `[scroll, scroll + viewport)`.
    fn adjust_scroll(&mut self, viewport: usize) {
        let viewport = viewport.max(1);
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + viewport {
            self.scroll = self.selected + 1 - viewport;
        }
    }

    fn render_all(&mut self) {
        self.rendered = self
            .rows
            .iter()
            .enumerate()
            .map(|(idx, row)| {
                render_row(
                    row,
                    &self.file_meta[idx],
                    idx == self.selected,
                    self.icons,
                    &self.theme,
                )
            })
            .collect();
    }
}

// ---------------------------------------------------------------------------
// Row rendering
// ---------------------------------------------------------------------------

/// Just the basename (last `/`-separated segment) of a path.
fn rename_leaf(path: &str) -> String {
    path.rsplit('/').next().unwrap_or(path).to_string()
}

fn render_row(
    row: &Row,
    meta: &FileRowMeta,
    selected: bool,
    icons: IconMode,
    theme: &Theme,
) -> Line<'static> {
    let mut spans = Vec::new();

    let bg = if selected {
        Some(rgb_to_color(theme.selection_bg))
    } else {
        None
    };
    let base = match bg {
        Some(c) => Style::default().bg(c),
        None => Style::default(),
    };

    match row {
        Row::Dir { label, depth } => {
            spans.push(Span::styled(indent(*depth), base));
            if icons == IconMode::On {
                spans.push(Span::styled(
                    format!("{} ", ICON_DIR_OPEN),
                    base.fg(rgb_to_color(theme.border)),
                ));
            }
            spans.push(Span::styled(
                label.clone(),
                base.fg(rgb_to_color(theme.muted))
                    .add_modifier(Modifier::BOLD),
            ));
        }
        Row::File { name, depth, .. } => {
            spans.push(Span::styled(indent(*depth), base));

            if let Some(status) = meta.status {
                let badge = format!("{} ", status.badge());
                spans.push(Span::styled(
                    badge,
                    base.fg(status_color(status)).add_modifier(Modifier::BOLD),
                ));
            }

            if icons == IconMode::On {
                let icon = file_icon(name);
                spans.push(Span::styled(
                    format!("{} ", icon),
                    base.fg(rgb_to_color(theme.border)),
                ));
            }

            let display_name = match &meta.rename {
                Some((old, new)) => format!("{old} \u{2192} {new}"),
                None => name.clone(),
            };
            spans.push(Span::styled(display_name, base));

            if let Some((added, deleted)) = meta.deltas {
                if added > 0 || deleted > 0 {
                    spans.push(Span::styled(" ".to_string(), base));
                }
                if added > 0 {
                    spans.push(Span::styled(format!("+{added}"), base.fg(Color::Green)));
                }
                if added > 0 && deleted > 0 {
                    spans.push(Span::styled(" ".to_string(), base));
                }
                if deleted > 0 {
                    spans.push(Span::styled(format!("-{deleted}"), base.fg(Color::Red)));
                }
            }
        }
    }

    Line::from(spans)
}

fn indent(depth: usize) -> String {
    "  ".repeat(depth)
}

fn status_color(status: FileStatus) -> Color {
    match status {
        FileStatus::Added => Color::Green,
        FileStatus::Deleted => Color::Red,
        FileStatus::Modified => Color::Yellow,
        FileStatus::Renamed => Color::Cyan,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn theme() -> Theme {
        Theme::default()
    }

    fn fd(path: &str) -> FileDiff {
        FileDiff {
            preamble: Vec::new(),
            old_path: path.to_string(),
            new_path: path.to_string(),
            rename_from: None,
            old_hash: None,
            new_hash: None,
            hunks: Vec::new(),
        }
    }

    fn fd_added(path: &str) -> FileDiff {
        FileDiff {
            preamble: Vec::new(),
            old_path: "/dev/null".to_string(),
            new_path: path.to_string(),
            rename_from: None,
            old_hash: Some("0".repeat(40)),
            new_hash: Some("a".repeat(40)),
            hunks: Vec::new(),
        }
    }

    fn fd_deleted(path: &str) -> FileDiff {
        FileDiff {
            preamble: Vec::new(),
            old_path: path.to_string(),
            new_path: "/dev/null".to_string(),
            rename_from: None,
            old_hash: Some("a".repeat(40)),
            new_hash: Some("0".repeat(40)),
            hunks: Vec::new(),
        }
    }

    fn fd_renamed(old: &str, new: &str) -> FileDiff {
        FileDiff {
            preamble: Vec::new(),
            old_path: old.to_string(),
            new_path: new.to_string(),
            rename_from: Some(old.to_string()),
            old_hash: Some("a".repeat(40)),
            new_hash: Some("b".repeat(40)),
            hunks: Vec::new(),
        }
    }

    fn line_text(line: &Line<'_>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    // --- file_status -------------------------------------------------

    #[test]
    fn file_status_classifies_added_file() {
        assert_eq!(file_status(&fd_added("a.rs")), FileStatus::Added);
    }

    #[test]
    fn file_status_classifies_deleted_file() {
        assert_eq!(file_status(&fd_deleted("a.rs")), FileStatus::Deleted);
    }

    #[test]
    fn file_status_classifies_modified_file() {
        let mut f = fd("a.rs");
        f.old_hash = Some("a".repeat(40));
        f.new_hash = Some("b".repeat(40));
        assert_eq!(file_status(&f), FileStatus::Modified);
    }

    #[test]
    fn file_status_classifies_renamed_file() {
        assert_eq!(
            file_status(&fd_renamed("old.rs", "new.rs")),
            FileStatus::Renamed
        );
    }

    // --- tree shape --------------------------------------------------

    #[test]
    fn build_rows_groups_files_under_common_directory() {
        let a = fd("src/a.rs");
        let b = fd("src/b.rs");
        let files = vec![
            SidebarFile {
                file: &a,
                added: 0,
                deleted: 0,
            },
            SidebarFile {
                file: &b,
                added: 0,
                deleted: 0,
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
            },
            SidebarFile {
                file: &b,
                added: 0,
                deleted: 0,
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
        // crates/{deltoids/src/lib.rs, deltoids-cli/src/main.rs} —
        // `crates/` has two children, must stay on its own row.
        let a = fd("crates/deltoids/src/lib.rs");
        let b = fd("crates/deltoids-cli/src/main.rs");
        let files = vec![
            SidebarFile {
                file: &a,
                added: 0,
                deleted: 0,
            },
            SidebarFile {
                file: &b,
                added: 0,
                deleted: 0,
            },
        ];
        let rows = build_rows(&files);
        // crates/ + deltoids/src/ + lib.rs + deltoids-cli/src/ + main.rs
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
            },
            SidebarFile {
                file: &b,
                added: 0,
                deleted: 0,
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
    fn build_rows_sorts_dirs_before_files_alphabetic() {
        let a = fd("zzz.rs");
        let b = fd("src/a.rs");
        let c = fd("aaa.rs");
        let files = vec![
            SidebarFile {
                file: &a,
                added: 0,
                deleted: 0,
            },
            SidebarFile {
                file: &b,
                added: 0,
                deleted: 0,
            },
            SidebarFile {
                file: &c,
                added: 0,
                deleted: 0,
            },
        ];
        let rows = build_rows(&files);
        // Expect: src/ ; src/a.rs ; aaa.rs ; zzz.rs
        match &rows[0] {
            Row::Dir { label, .. } => assert_eq!(label, "src/"),
            other => panic!("expected src/ first, got {other:?}"),
        }
        match &rows[2] {
            Row::File { name, .. } => assert_eq!(name, "aaa.rs"),
            other => panic!("expected aaa.rs after src/, got {other:?}"),
        }
        match &rows[3] {
            Row::File { name, .. } => assert_eq!(name, "zzz.rs"),
            other => panic!("expected zzz.rs last, got {other:?}"),
        }
    }

    // --- icons -------------------------------------------------------

    #[test]
    fn file_icon_known_extensions() {
        assert_eq!(file_icon("lib.rs"), "\u{e7a8}");
        assert_eq!(file_icon("doc.md"), "\u{f48a}");
        assert_eq!(file_icon("Cargo.toml"), "\u{e7a8}"); // exact match wins
        assert_eq!(file_icon("config.json"), "\u{e60b}");
    }

    #[test]
    fn file_icon_unknown_extension_falls_back() {
        assert_eq!(file_icon("strange.xyz"), "\u{f15b}");
        assert_eq!(file_icon("noextension"), "\u{f15b}");
    }

    // --- selection ---------------------------------------------------

    #[test]
    fn build_selects_first_file_skipping_dir_header() {
        let a = fd("src/a.rs");
        let files = vec![SidebarFile {
            file: &a,
            added: 0,
            deleted: 0,
        }];
        let sidebar = Sidebar::build_with_icons(&files, &theme(), IconMode::Off);
        // rows: src/ , a.rs — selected should be 1
        assert_eq!(sidebar.selected(), 1);
        assert_eq!(sidebar.selected_file_index(), Some(0));
    }

    #[test]
    fn move_down_advances_one_row_including_dirs() {
        // Layout:
        //   crates/                      row 0 dir
        //     deltoids/src/              row 1 dir
        //       lib.rs                   row 2 file
        //     deltoids-cli/src/          row 3 dir
        //       main.rs                  row 4 file
        let a = fd("crates/deltoids/src/lib.rs");
        let b = fd("crates/deltoids-cli/src/main.rs");
        let files = vec![
            SidebarFile {
                file: &a,
                added: 0,
                deleted: 0,
            },
            SidebarFile {
                file: &b,
                added: 0,
                deleted: 0,
            },
        ];
        let mut sidebar = Sidebar::build_with_icons(&files, &theme(), IconMode::Off);
        // Initial selection still lands on the first file row, so the
        // diff has something useful to snap to on startup.
        let first = sidebar.selected();
        assert!(
            !sidebar.selected_is_dir(),
            "initial selection must be a file row"
        );
        // From the first file, moving up walks back into directory
        // headers one row at a time.
        sidebar.move_up(20);
        assert_eq!(sidebar.selected(), first - 1);
        assert!(
            sidebar.selected_is_dir(),
            "move_up from a file should land on its parent dir row"
        );
        // Moving down again returns to the file.
        sidebar.move_down(20);
        assert_eq!(sidebar.selected(), first);
        assert!(!sidebar.selected_is_dir());
    }

    #[test]
    fn move_down_at_last_file_is_noop() {
        let a = fd("a.rs");
        let files = vec![SidebarFile {
            file: &a,
            added: 0,
            deleted: 0,
        }];
        let mut sidebar = Sidebar::build_with_icons(&files, &theme(), IconMode::Off);
        let before = sidebar.selected();
        sidebar.move_down(20);
        assert_eq!(sidebar.selected(), before);
    }

    #[test]
    fn move_up_with_no_dirs_above_is_noop() {
        // Top-level file: there's no row above row 0, so move_up has
        // nowhere to go.
        let a = fd("a.rs");
        let files = vec![SidebarFile {
            file: &a,
            added: 0,
            deleted: 0,
        }];
        let mut sidebar = Sidebar::build_with_icons(&files, &theme(), IconMode::Off);
        let before = sidebar.selected();
        sidebar.move_up(20);
        assert_eq!(sidebar.selected(), before);
    }

    #[test]
    fn top_jumps_to_first_row_and_bottom_jumps_to_last() {
        let a = fd("a/x.rs");
        let b = fd("b/y.rs");
        let c = fd("c/z.rs");
        let files = vec![
            SidebarFile {
                file: &a,
                added: 0,
                deleted: 0,
            },
            SidebarFile {
                file: &b,
                added: 0,
                deleted: 0,
            },
            SidebarFile {
                file: &c,
                added: 0,
                deleted: 0,
            },
        ];
        let mut sidebar = Sidebar::build_with_icons(&files, &theme(), IconMode::Off);
        sidebar.bottom(20);
        // Last row is the last file (c/z.rs file), since each dir is
        // followed by its single file leaf.
        assert_eq!(sidebar.selected(), sidebar.row_count() - 1);
        assert!(!sidebar.selected_is_dir());
        sidebar.top(20);
        // First row is the first directory header.
        assert_eq!(sidebar.selected(), 0);
        assert!(sidebar.selected_is_dir());
    }

    #[test]
    fn nearest_file_index_on_dir_returns_first_file_in_subtree() {
        // Layout: src/{a.rs,b.rs}, top-level c.rs.
        // Rows: src/ ; src/a.rs ; src/b.rs ; c.rs.
        let a = fd("src/a.rs");
        let b = fd("src/b.rs");
        let c = fd("c.rs");
        let files = vec![
            SidebarFile {
                file: &a,
                added: 0,
                deleted: 0,
            },
            SidebarFile {
                file: &b,
                added: 0,
                deleted: 0,
            },
            SidebarFile {
                file: &c,
                added: 0,
                deleted: 0,
            },
        ];
        let mut sidebar = Sidebar::build_with_icons(&files, &theme(), IconMode::Off);
        // Land on the src/ header.
        sidebar.top(20);
        assert!(sidebar.selected_is_dir());
        // nearest_file_index points at src/a.rs (input index 0).
        assert_eq!(sidebar.nearest_file_index(), Some(0));
        // Move down to a.rs; nearest is itself.
        sidebar.move_down(20);
        assert_eq!(sidebar.nearest_file_index(), Some(0));
        // Then b.rs.
        sidebar.move_down(20);
        assert_eq!(sidebar.nearest_file_index(), Some(1));
        // Then c.rs (top-level file).
        sidebar.move_down(20);
        assert_eq!(sidebar.nearest_file_index(), Some(2));
    }

    #[test]
    fn selected_file_index_returns_none_on_dir_row() {
        let a = fd("src/a.rs");
        let files = vec![SidebarFile {
            file: &a,
            added: 0,
            deleted: 0,
        }];
        let mut sidebar = Sidebar::build_with_icons(&files, &theme(), IconMode::Off);
        sidebar.top(20);
        assert!(sidebar.selected_is_dir());
        assert_eq!(sidebar.selected_file_index(), None);
        // nearest_file_index still finds the file under it.
        assert_eq!(sidebar.nearest_file_index(), Some(0));
    }

    #[test]
    fn subtree_display_range_is_none_when_file_selected() {
        let a = fd("src/a.rs");
        let files = vec![SidebarFile {
            file: &a,
            added: 0,
            deleted: 0,
        }];
        let sidebar = Sidebar::build_with_icons(&files, &theme(), IconMode::Off);
        // Initial selection lands on the file row.
        assert!(!sidebar.selected_is_dir());
        assert_eq!(sidebar.subtree_display_range(), None);
    }

    #[test]
    fn subtree_display_range_covers_files_under_top_level_dir() {
        // Layout:
        //   src/                     (dir 0)
        //     a.rs                   (file 0)
        //     b.rs                   (file 1)
        //   util/                    (dir 2)
        //     c.rs                   (file 2)
        let a = fd("src/a.rs");
        let b = fd("src/b.rs");
        let c = fd("util/c.rs");
        let files = vec![
            SidebarFile {
                file: &a,
                added: 0,
                deleted: 0,
            },
            SidebarFile {
                file: &b,
                added: 0,
                deleted: 0,
            },
            SidebarFile {
                file: &c,
                added: 0,
                deleted: 0,
            },
        ];
        let mut sidebar = Sidebar::build_with_icons(&files, &theme(), IconMode::Off);
        // Land on src/.
        sidebar.top(20);
        assert!(sidebar.selected_is_dir());
        assert_eq!(sidebar.subtree_display_range(), Some(0..2));

        // Land on util/ (dir 2 in row order, after src/, a.rs, b.rs).
        sidebar.move_down(20); // a.rs
        sidebar.move_down(20); // b.rs
        sidebar.move_down(20); // util/
        assert!(sidebar.selected_is_dir());
        assert_eq!(sidebar.subtree_display_range(), Some(2..3));
    }

    #[test]
    fn subtree_display_range_covers_nested_subtrees() {
        // Layout (from earlier example, multiple subdirs under crates/):
        //   crates/                          depth 0
        //     deltoids/                      depth 1
        //       src/                         depth 2
        //         lib.rs                     depth 3 (file 0 in display)
        //     deltoids-cli/                  depth 1
        //       src/                         depth 2
        //         main.rs                    depth 3 (file 1 in display)
        let a = fd("crates/deltoids/src/lib.rs");
        let b = fd("crates/deltoids-cli/src/main.rs");
        let files = vec![
            SidebarFile {
                file: &a,
                added: 0,
                deleted: 0,
            },
            SidebarFile {
                file: &b,
                added: 0,
                deleted: 0,
            },
        ];
        let mut sidebar = Sidebar::build_with_icons(&files, &theme(), IconMode::Off);
        // Land on crates/ (row 0). Subtree includes both files.
        sidebar.top(20);
        assert_eq!(sidebar.subtree_display_range(), Some(0..2));

        // Move to deltoids/ (depth 1). Subtree includes only lib.rs (file 0).
        sidebar.move_down(20);
        assert!(sidebar.selected_is_dir());
        assert_eq!(sidebar.subtree_display_range(), Some(0..1));
    }

    #[test]
    fn page_down_advances_by_viewport_rows() {
        // 8 top-level files, viewport 3. From the initial selection
        // (row 0), page_down(3) should land on row 3, then row 6, then
        // clamp at the last row (7).
        let owned: Vec<FileDiff> = (0..8).map(|i| fd(&format!("f{i}.rs"))).collect();
        let files: Vec<_> = owned
            .iter()
            .map(|f| SidebarFile {
                file: f,
                added: 0,
                deleted: 0,
            })
            .collect();
        let mut sidebar = Sidebar::build_with_icons(&files, &theme(), IconMode::Off);
        assert_eq!(sidebar.selected(), 0);
        sidebar.page_down(3);
        assert_eq!(sidebar.selected(), 3);
        sidebar.page_down(3);
        assert_eq!(sidebar.selected(), 6);
        sidebar.page_down(3);
        assert_eq!(sidebar.selected(), 7);
        sidebar.page_up(3);
        assert_eq!(sidebar.selected(), 4);
    }

    // --- scroll tracking ---------------------------------------------

    #[test]
    fn scroll_keeps_selection_in_view_on_move_down() {
        // 8 files, viewport 3.  Moving down past row 3 should bump scroll.
        let owned: Vec<FileDiff> = (0..8).map(|i| fd(&format!("f{i}.rs"))).collect();
        let files: Vec<_> = owned
            .iter()
            .map(|f| SidebarFile {
                file: f,
                added: 0,
                deleted: 0,
            })
            .collect();
        let mut sidebar = Sidebar::build_with_icons(&files, &theme(), IconMode::Off);
        assert_eq!(sidebar.scroll(), 0);
        for _ in 0..7 {
            sidebar.move_down(3);
        }
        // Selected must be visible: scroll <= selected < scroll + 3.
        let s = sidebar.selected();
        let scroll = sidebar.scroll();
        assert!(
            scroll <= s && s < scroll + 3,
            "selection {s} not in viewport [{scroll}, {})",
            scroll + 3
        );
    }

    // --- rendering ---------------------------------------------------

    #[test]
    fn rendered_file_row_contains_status_and_name() {
        let f = fd_added("hello.rs");
        let files = vec![SidebarFile {
            file: &f,
            added: 12,
            deleted: 0,
        }];
        let sidebar = Sidebar::build_with_icons(&files, &theme(), IconMode::Off);
        let rows = sidebar.rows();
        // Find the file row.
        let file_row = rows
            .iter()
            .find(|r| line_text(r).contains("hello.rs"))
            .expect("file row not found");
        let text = line_text(file_row);
        assert!(text.contains('A'), "missing status A in {text:?}");
        assert!(text.contains("+12"), "missing +12 in {text:?}");
    }

    #[test]
    fn rendered_renamed_file_shows_old_arrow_new() {
        let f = fd_renamed("src/old.rs", "src/new.rs");
        let files = vec![SidebarFile {
            file: &f,
            added: 1,
            deleted: 1,
        }];
        let sidebar = Sidebar::build_with_icons(&files, &theme(), IconMode::Off);
        let combined = sidebar
            .rows()
            .iter()
            .map(line_text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            combined.contains("old.rs \u{2192} new.rs"),
            "missing rename arrow in: {combined}"
        );
    }

    #[test]
    fn rendered_directory_row_contains_label() {
        let f = fd("src/a.rs");
        let files = vec![SidebarFile {
            file: &f,
            added: 0,
            deleted: 0,
        }];
        let sidebar = Sidebar::build_with_icons(&files, &theme(), IconMode::Off);
        let rows = sidebar.rows();
        let dir = rows.first().expect("at least one row");
        assert!(line_text(dir).contains("src/"));
    }

    #[test]
    fn icons_off_omits_icon_glyphs() {
        let f = fd("a.rs");
        let files = vec![SidebarFile {
            file: &f,
            added: 0,
            deleted: 0,
        }];
        let sidebar = Sidebar::build_with_icons(&files, &theme(), IconMode::Off);
        let row = &sidebar.rows()[0];
        let text = line_text(row);
        assert!(
            !text.contains('\u{e7a8}') && !text.contains('\u{f15b}'),
            "expected no file icon, got {text:?}"
        );
    }

    #[test]
    fn icons_on_includes_icon_glyph_for_known_extension() {
        let f = fd("a.rs");
        let files = vec![SidebarFile {
            file: &f,
            added: 0,
            deleted: 0,
        }];
        let sidebar = Sidebar::build_with_icons(&files, &theme(), IconMode::On);
        let combined = sidebar
            .rows()
            .iter()
            .map(line_text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(combined.contains('\u{e7a8}'), "missing rust icon");
    }

    #[test]
    fn empty_files_produces_empty_sidebar() {
        let sidebar = Sidebar::build_with_icons(&[], &theme(), IconMode::Off);
        assert_eq!(sidebar.row_count(), 0);
        assert!(sidebar.rows().is_empty());
        assert_eq!(sidebar.selected_file_index(), None);
    }
}

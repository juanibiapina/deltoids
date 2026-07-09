//! Row-rendering axis: turn one [`Row`] plus its [`FileRowMeta`] into a
//! styled [`Line`], including status badges, icons, delta counts, and the
//! trailing binary/mode/submodule badges.

use deltoids::Theme;
use deltoids::parse::FileDiff;
use deltoids::render_tui::rgb_to_color;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use super::FileRowMeta;
use super::icons::{IconMode, dir_icon, file_icon};
use super::status::{FileMode, FileStatus, ModeChange};
use super::tree::Row;

/// Just the basename (last `/`-separated segment) of a path.
pub(super) fn rename_leaf(path: &str) -> String {
    path.rsplit('/').next().unwrap_or(path).to_string()
}

/// Pull the source path from a `copy from` line in the preamble.
pub(super) fn copy_origin(file: &FileDiff) -> Option<String> {
    file.preamble.iter().find_map(|line| {
        line.trim_start()
            .strip_prefix("copy from ")
            .map(str::to_owned)
    })
}

/// Short label describing a [`ModeChange`].
///
/// Executable bit toggles render as `+x` / `-x`. Real type changes
/// render as `regular→symlink`, etc., so the user sees what the file
/// turned into.
fn mode_change_label(change: ModeChange) -> String {
    match change {
        ModeChange::ExecutableSet => "+x".to_string(),
        ModeChange::ExecutableCleared => "-x".to_string(),
        ModeChange::TypeChange { old, new } => {
            format!("{}→{}", file_mode_label(old), file_mode_label(new))
        }
    }
}

fn file_mode_label(mode: FileMode) -> &'static str {
    match mode {
        FileMode::Regular => "file",
        FileMode::Executable => "exec",
        FileMode::Symlink => "symlink",
        FileMode::Submodule => "submodule",
        FileMode::Other => "?",
    }
}

pub(super) fn render_row(
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
    let mut base = match bg {
        Some(c) => Style::default().bg(c),
        None => Style::default(),
    };
    // Bold the whole selected row (lazygit parity): every span derives
    // from `base`, so the modifier carries through icons, status, and
    // name.
    if selected {
        base = base.add_modifier(Modifier::BOLD);
    }

    match row {
        Row::Dir { label, depth } => {
            dir_row_spans(&mut spans, label, *depth, meta, icons, theme, base)
        }
        Row::File { name, depth, .. } => {
            file_row_spans(&mut spans, name, *depth, meta, icons, theme, base)
        }
    }

    Line::from(spans)
}

#[allow(clippy::too_many_arguments)]
fn dir_row_spans(
    spans: &mut Vec<Span<'static>>,
    label: &str,
    depth: usize,
    meta: &FileRowMeta,
    icons: IconMode,
    theme: &Theme,
    base: Style,
) {
    spans.push(Span::styled(indent(depth), base));
    if icons == IconMode::On {
        // Look the icon up by the deepest segment of the label (strip the
        // trailing `/`, take the last path component), since that segment
        // is the directory the row's contents live in.
        let key = label
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or(label);
        let icon = dir_icon(key);
        spans.push(Span::styled(
            format!("{} ", icon.glyph),
            base.fg(rgb_to_color(icon.color)),
        ));
    }
    // With stage data, tint the label green/yellow/default by the
    // subtree's aggregate staging state (lazygit parity). Without it
    // (piped diff / no repo) keep the muted, bold directory styling.
    let label_style = match meta.dir_stage {
        Some(agg) => match stage_tint(agg.has_staged, agg.has_unstaged, theme) {
            Some(color) => base.fg(color),
            None => base,
        },
        None => base
            .fg(rgb_to_color(theme.muted))
            .add_modifier(Modifier::BOLD),
    };
    spans.push(Span::styled(label.to_string(), label_style));
}

#[allow(clippy::too_many_arguments)]
fn file_row_spans(
    spans: &mut Vec<Span<'static>>,
    name: &str,
    depth: usize,
    meta: &FileRowMeta,
    icons: IconMode,
    theme: &Theme,
    base: Style,
) {
    spans.push(Span::styled(indent(depth), base));

    match meta.stage {
        // Two-column stage field (lazygit parity): staged column then
        // worktree column, each in its own colour.
        Some(stage) => {
            // Untracked renders porcelain `??`: the staging model keeps
            // the staged column empty (an untracked file is not staged),
            // but display convention shows `?` in both columns.
            let is_untracked = stage.staged.is_none()
                && stage.unstaged == Some(super::status::ChangeKind::Untracked);
            let staged_col = if is_untracked {
                Some(super::status::ChangeKind::Untracked)
            } else {
                stage.staged
            };
            spans.push(stage_char_span(
                staged_col,
                StageColumn::Staged,
                base,
                theme,
            ));
            spans.push(stage_char_span(
                stage.unstaged,
                StageColumn::Worktree,
                base,
                theme,
            ));
            spans.push(Span::styled(" ".to_string(), base));
        }
        // Fallback: single change-type letter from the combined diff
        // (piped diff / no repo).
        None => {
            if let Some(status) = meta.status {
                let badge = format!("{} ", status.badge());
                spans.push(Span::styled(
                    badge,
                    base.fg(status_color(status, theme))
                        .add_modifier(Modifier::BOLD),
                ));
            }
        }
    }

    if icons == IconMode::On {
        let icon = file_icon(name);
        spans.push(Span::styled(
            format!("{} ", icon.glyph),
            base.fg(rgb_to_color(icon.color)),
        ));
    }

    let display_name = match &meta.rename {
        Some((old, new)) => format!("{old} \u{2192} {new}"),
        None => name.to_string(),
    };
    // Filename colour. With stage data, follow lazygit's `getFileLine`:
    // green when fully staged, yellow when partially staged, default
    // otherwise. Without stage data (piped diff / no repo) fall back to
    // greening a fully-added file, matching lazygit's staged-add
    // treatment.
    let name_style = match meta.stage {
        Some(stage) => match stage_tint(stage.is_staged(), stage.is_unstaged(), theme) {
            Some(color) => base.fg(color),
            None => base,
        },
        None if meta.status == Some(FileStatus::Added) => base.fg(rgb_to_color(theme.status_added)),
        None => base,
    };
    spans.push(Span::styled(display_name, name_style));

    if let Some((added, deleted)) = meta.deltas {
        if added > 0 || deleted > 0 {
            spans.push(Span::styled(" ".to_string(), base));
        }
        if added > 0 {
            spans.push(Span::styled(
                format!("+{added}"),
                base.fg(rgb_to_color(theme.status_added)),
            ));
        }
        if added > 0 && deleted > 0 {
            spans.push(Span::styled(" ".to_string(), base));
        }
        if deleted > 0 {
            spans.push(Span::styled(
                format!("-{deleted}"),
                base.fg(rgb_to_color(theme.status_deleted)),
            ));
        }
    }

    // Trailing badges: binary, mode change, submodule. These sit at the
    // right of the row in the muted/border colour so they don't compete
    // with the status badge.
    let muted = base.fg(rgb_to_color(theme.muted));
    if meta.extra.binary {
        spans.push(Span::styled(" (binary)".to_string(), muted));
    }
    if let Some(change) = meta.extra.mode_change {
        spans.push(Span::styled(
            format!(" ({})", mode_change_label(change)),
            muted,
        ));
    }
    if meta.extra.is_submodule {
        spans.push(Span::styled(" (submodule)".to_string(), muted));
    }
}

fn indent(depth: usize) -> String {
    "  ".repeat(depth)
}

/// Which porcelain column a stage character belongs to. The column
/// governs the default colour (staged = green, worktree = red).
#[derive(Clone, Copy)]
enum StageColumn {
    Staged,
    Worktree,
}

/// One character of the two-column stage field: the change letter (or a
/// space when the column is empty), coloured by lazygit's rule. The
/// staged column is green, the worktree column red; an untracked `?` is
/// red regardless of column; an empty column takes the name colour.
fn stage_char_span(
    change: Option<super::status::ChangeKind>,
    column: StageColumn,
    base: Style,
    theme: &Theme,
) -> Span<'static> {
    match change {
        None => Span::styled(" ".to_string(), base),
        Some(kind) => {
            let color = if kind == super::status::ChangeKind::Untracked {
                rgb_to_color(theme.status_deleted)
            } else {
                match column {
                    StageColumn::Staged => rgb_to_color(theme.status_added),
                    StageColumn::Worktree => rgb_to_color(theme.status_deleted),
                }
            };
            Span::styled(
                kind.letter().to_string(),
                base.fg(color).add_modifier(Modifier::BOLD),
            )
        }
    }
}

/// Lazygit's label-tint rule, shared by file rows and directory rows.
/// Green when fully staged, yellow when partially staged, `None`
/// (terminal default) otherwise. For a directory the inputs are the
/// OR-fold over its subtree; for a file they're its own two columns.
fn stage_tint(has_staged: bool, has_unstaged: bool, theme: &Theme) -> Option<Color> {
    match (has_staged, has_unstaged) {
        (true, false) => Some(rgb_to_color(theme.status_added)),
        (true, true) => Some(rgb_to_color(theme.status_partial)),
        _ => None,
    }
}

// Status-letter colours mirror lazygit's working-tree Files panel, where the
// worktree column is red: a modified or deleted file shows a red letter, an
// added file green. deltoids has no staged/unstaged axis, so it collapses
// lazygit's two porcelain columns into this single change-type letter.
fn status_color(status: FileStatus, theme: &Theme) -> Color {
    let rgb = match status {
        FileStatus::Added => theme.status_added,
        FileStatus::Deleted => theme.status_deleted,
        FileStatus::Modified => theme.status_modified,
        FileStatus::Renamed => theme.status_partial,
        FileStatus::Copied => theme.status_copied,
        FileStatus::TypeChanged => theme.status_typechange,
    };
    rgb_to_color(rgb)
}

#[cfg(test)]
mod tests {
    use super::super::{Sidebar, SidebarFile};
    use super::*;
    use crate::sidebar::test_support::*;

    #[test]
    fn rendered_binary_file_row_has_binary_badge() {
        let f = fd_with_preamble(
            "image.png",
            &["Binary files a/image.png and b/image.png differ"],
        );
        let files = vec![SidebarFile {
            file: &f,
            added: 0,
            deleted: 0,
            stage: None,
        }];
        let sidebar = Sidebar::build_with_icons(&files, &theme(), IconMode::Off);
        let combined = sidebar
            .rows()
            .iter()
            .map(line_text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            combined.contains("(binary)"),
            "missing binary badge: {combined}"
        );
        assert!(
            !combined.contains('+') && !combined.contains('-'),
            "binary row must not show +/- counts: {combined}"
        );
    }

    #[test]
    fn rendered_mode_change_row_has_executable_badge() {
        let f = fd_with_preamble("script.sh", &["old mode 100644", "new mode 100755"]);
        let files = vec![SidebarFile {
            file: &f,
            added: 0,
            deleted: 0,
            stage: None,
        }];
        let sidebar = Sidebar::build_with_icons(&files, &theme(), IconMode::Off);
        let combined = sidebar
            .rows()
            .iter()
            .map(line_text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(combined.contains("(+x)"), "missing +x badge: {combined}");
    }

    #[test]
    fn rendered_copied_file_shows_old_arrow_new_and_c_status() {
        let mut f = fd("new.rs");
        f.preamble = vec![
            "diff --git a/old.rs b/new.rs".to_string(),
            "similarity index 100%".to_string(),
            "copy from old.rs".to_string(),
            "copy to new.rs".to_string(),
        ];
        f.old_hash = Some("a".repeat(40));
        f.new_hash = Some("b".repeat(40));
        let files = vec![SidebarFile {
            file: &f,
            added: 0,
            deleted: 0,
            stage: None,
        }];
        let sidebar = Sidebar::build_with_icons(&files, &theme(), IconMode::Off);
        let combined = sidebar
            .rows()
            .iter()
            .map(line_text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(combined.contains('C'), "missing C status: {combined}");
        assert!(
            combined.contains("old.rs \u{2192} new.rs"),
            "missing copy arrow: {combined}"
        );
    }

    #[test]
    fn rendered_file_row_contains_status_and_name() {
        let f = fd_added("hello.rs");
        let files = vec![SidebarFile {
            file: &f,
            added: 12,
            deleted: 0,
            stage: None,
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
            stage: None,
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
    fn rendered_type_change_row_shows_t_badge_counts_and_mode_label() {
        let f = fd_with_preamble("f.txt", &["old mode 100644", "new mode 120000"]);
        let files = vec![SidebarFile {
            file: &f,
            added: 1,
            deleted: 2,
            stage: Some(StageStatus {
                staged: Some(ChangeKind::TypeChanged),
                unstaged: None,
            }),
        }];
        let sidebar = Sidebar::build_with_icons(&files, &theme(), IconMode::Off);
        let spans = &sidebar.rows()[0].spans;

        // Staged column carries the single `T`.
        let [(sc, _), (wc, _)] = stage_chars(spans);
        assert_eq!((sc, wc), ('T', ' '));

        let combined = spans.iter().map(|s| s.content.as_ref()).collect::<String>();
        assert!(
            combined.contains("(file\u{2192}symlink)"),
            "missing type-change badge in: {combined}"
        );
        assert!(combined.contains("+1"), "missing +1 in: {combined}");
        assert!(combined.contains("-2"), "missing -2 in: {combined}");
    }

    #[test]
    fn rendered_directory_row_contains_label() {
        let f = fd("src/a.rs");
        let files = vec![SidebarFile {
            file: &f,
            added: 0,
            deleted: 0,
            stage: None,
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
            stage: None,
        }];
        let sidebar = Sidebar::build_with_icons(&files, &theme(), IconMode::Off);
        let row = &sidebar.rows()[0];
        let text = line_text(row);
        assert!(
            !text.contains('\u{e68b}') && !text.contains('\u{f15b}'),
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
            stage: None,
        }];
        let sidebar = Sidebar::build_with_icons(&files, &theme(), IconMode::On);
        let combined = sidebar
            .rows()
            .iter()
            .map(line_text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(combined.contains('\u{e68b}'), "missing rust icon");
    }

    #[test]
    fn rust_file_row_uses_rust_icon_color() {
        let f = fd("a.rs");
        let files = vec![SidebarFile {
            file: &f,
            added: 0,
            deleted: 0,
            stage: None,
        }];
        let sidebar = Sidebar::build_with_icons(&files, &theme(), IconMode::On);
        let row = &sidebar.rows()[0];
        // The icon span carries the rust glyph in lazygit's rust colour,
        // not the flat theme.border colour.
        let icon_span = row
            .spans
            .iter()
            .find(|s| s.content.contains('\u{e68b}'))
            .expect("rust icon span");
        assert_eq!(icon_span.style.fg, Some(Color::Rgb(0xFF, 0x70, 0x43)));
    }

    use super::super::{ChangeKind, StageStatus};

    /// Build a one-file sidebar with an explicit stage status and return
    /// its single file row's spans.
    fn stage_row(f: &FileDiff, stage: StageStatus) -> Vec<Span<'static>> {
        let files = vec![SidebarFile {
            file: f,
            added: 0,
            deleted: 0,
            stage: Some(stage),
        }];
        let sidebar = Sidebar::build_with_icons(&files, &theme(), IconMode::Off);
        sidebar.rows()[0].spans.clone()
    }

    /// The first two spans after the leading indent are the stage
    /// characters (staged, worktree). Return them as (char, fg).
    fn stage_chars(spans: &[Span<'static>]) -> [(char, Option<Color>); 2] {
        // spans[0] is the indent (empty for a top-level file).
        let staged = &spans[1];
        let worktree = &spans[2];
        [
            (staged.content.chars().next().unwrap(), staged.style.fg),
            (worktree.content.chars().next().unwrap(), worktree.style.fg),
        ]
    }

    fn name_span<'a>(spans: &'a [Span<'static>], name: &str) -> &'a Span<'static> {
        spans.iter().find(|s| s.content == name).expect("name span")
    }

    #[test]
    fn stage_field_staged_add_shows_a_space() {
        let f = fd("added.rs");
        let spans = stage_row(
            &f,
            StageStatus {
                staged: Some(ChangeKind::Added),
                unstaged: None,
            },
        );
        let [(sc, sfg), (wc, _)] = stage_chars(&spans);
        assert_eq!(sc, 'A');
        assert_eq!(sfg, Some(rgb_to_color(theme().status_added)));
        assert_eq!(wc, ' ');
        // Fully staged: filename is green.
        assert_eq!(
            name_span(&spans, "added.rs").style.fg,
            Some(rgb_to_color(theme().status_added))
        );
    }

    #[test]
    fn stage_field_unstaged_modify_shows_space_m() {
        let f = fd("mod.rs");
        let spans = stage_row(
            &f,
            StageStatus {
                staged: None,
                unstaged: Some(ChangeKind::Modified),
            },
        );
        let [(sc, _), (wc, wfg)] = stage_chars(&spans);
        assert_eq!(sc, ' ');
        assert_eq!(wc, 'M');
        assert_eq!(wfg, Some(rgb_to_color(theme().status_deleted)));
        // Not staged: default filename colour (no explicit fg).
        assert_eq!(name_span(&spans, "mod.rs").style.fg, None);
    }

    #[test]
    fn stage_field_staged_then_edited_shows_mm_yellow_name() {
        let f = fd("both.rs");
        let spans = stage_row(
            &f,
            StageStatus {
                staged: Some(ChangeKind::Modified),
                unstaged: Some(ChangeKind::Modified),
            },
        );
        let [(sc, sfg), (wc, wfg)] = stage_chars(&spans);
        assert_eq!((sc, wc), ('M', 'M'));
        assert_eq!(sfg, Some(rgb_to_color(theme().status_added)));
        assert_eq!(wfg, Some(rgb_to_color(theme().status_deleted)));
        // Partially staged: filename is yellow.
        assert_eq!(
            name_span(&spans, "both.rs").style.fg,
            Some(rgb_to_color(theme().status_partial))
        );
    }

    #[test]
    fn stage_field_untracked_shows_double_question_red() {
        let f = fd("new.rs");
        let spans = stage_row(
            &f,
            StageStatus {
                staged: None,
                unstaged: Some(ChangeKind::Untracked),
            },
        );
        // Untracked renders porcelain `??`: `?` in both columns, both red.
        let [(sc, sfg), (wc, wfg)] = stage_chars(&spans);
        assert_eq!((sc, wc), ('?', '?'));
        assert_eq!(sfg, Some(rgb_to_color(theme().status_deleted)));
        assert_eq!(wfg, Some(rgb_to_color(theme().status_deleted)));
        // The staging model stays porcelain (staged column empty), so the
        // name is not tinted as staged.
        assert_eq!(name_span(&spans, "new.rs").style.fg, None);
    }

    #[test]
    fn custom_theme_status_colors_flow_into_spans() {
        // A theme with distinctive status colours proves the row path reads
        // theme fields rather than hardcoded ANSI: if colours were still
        // hardcoded, these spans would carry the default RGBs instead.
        let mut custom = theme();
        custom.status_added = (1, 2, 3);
        custom.status_deleted = (4, 5, 6);
        custom.status_partial = (7, 8, 9);

        // Fully staged add: stage char + name tint use status_added.
        let f = fd("added.rs");
        let files = vec![SidebarFile {
            file: &f,
            added: 5,
            deleted: 2,
            stage: Some(StageStatus {
                staged: Some(ChangeKind::Added),
                unstaged: None,
            }),
        }];
        let sidebar = Sidebar::build_with_icons(&files, &custom, IconMode::Off);
        let spans = &sidebar.rows()[0].spans;
        let [(_, sfg), _] = stage_chars(spans);
        assert_eq!(sfg, Some(rgb_to_color(custom.status_added)));
        assert_eq!(
            name_span(spans, "added.rs").style.fg,
            Some(rgb_to_color(custom.status_added))
        );
        // +N uses status_added, -N uses status_deleted.
        let plus = spans
            .iter()
            .find(|s| s.content.as_ref() == "+5")
            .expect("+5 span");
        let minus = spans
            .iter()
            .find(|s| s.content.as_ref() == "-2")
            .expect("-2 span");
        assert_eq!(plus.style.fg, Some(rgb_to_color(custom.status_added)));
        assert_eq!(minus.style.fg, Some(rgb_to_color(custom.status_deleted)));
    }

    #[test]
    fn stage_none_uses_single_letter_fallback() {
        // No stage data: the piped-diff path keeps the single change-type
        // letter derived from the diff (here an added file → 'A').
        let f = fd_added("a.rs");
        let files = vec![SidebarFile {
            file: &f,
            added: 0,
            deleted: 0,
            stage: None,
        }];
        let sidebar = Sidebar::build_with_icons(&files, &theme(), IconMode::Off);
        let spans = &sidebar.rows()[0].spans;
        // Second span (after indent) is the "A " badge, not a 1-char col.
        assert_eq!(spans[1].content.as_ref(), "A ");
        assert_eq!(spans[1].style.fg, Some(rgb_to_color(theme().status_added)));
    }

    #[test]
    fn selected_row_is_bold_and_unselected_is_not() {
        // Two top-level files; the first is selected on build.
        let a = fd_added("a.rs");
        let b = fd_added("b.rs");
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
        let sidebar = Sidebar::build_with_icons(&files, &theme(), IconMode::Off);
        let selected = &sidebar.rows()[sidebar.selected()];
        let other_idx = if sidebar.selected() == 0 { 1 } else { 0 };
        let other = &sidebar.rows()[other_idx];
        // The name span is not otherwise bold (added-file name is a plain
        // green span), so it isolates the selection modifier.
        assert!(
            name_span(&selected.spans, "a.rs")
                .style
                .add_modifier
                .contains(Modifier::BOLD),
            "selected row name should be bold"
        );
        assert!(
            !name_span(&other.spans, "b.rs")
                .style
                .add_modifier
                .contains(Modifier::BOLD),
            "unselected row name should not be bold"
        );
    }

    #[test]
    fn stage_tint_matches_lazygit_rule() {
        let theme = theme();
        assert_eq!(
            stage_tint(true, false, &theme),
            Some(rgb_to_color(theme.status_added))
        );
        assert_eq!(
            stage_tint(true, true, &theme),
            Some(rgb_to_color(theme.status_partial))
        );
        assert_eq!(stage_tint(false, true, &theme), None);
        assert_eq!(stage_tint(false, false, &theme), None);
    }

    /// A `SidebarFile` at `path` with an explicit stage status.
    fn staged_file<'a>(f: &'a FileDiff, stage: StageStatus) -> SidebarFile<'a> {
        SidebarFile {
            file: f,
            added: 0,
            deleted: 0,
            stage: Some(stage),
        }
    }

    /// The fg of the directory label span (the span carrying `label`).
    fn dir_label_span<'a>(row: &'a Line<'static>, label: &str) -> &'a Span<'static> {
        row.spans
            .iter()
            .find(|s| s.content.as_ref() == label)
            .expect("dir label span")
    }

    #[test]
    fn dir_with_only_staged_children_is_green() {
        let a = fd("src/a.rs");
        let b = fd("src/b.rs");
        let files = vec![
            staged_file(
                &a,
                StageStatus {
                    staged: Some(ChangeKind::Added),
                    unstaged: None,
                },
            ),
            staged_file(
                &b,
                StageStatus {
                    staged: Some(ChangeKind::Modified),
                    unstaged: None,
                },
            ),
        ];
        let sidebar = Sidebar::build_with_icons(&files, &theme(), IconMode::Off);
        let span = dir_label_span(&sidebar.rows()[0], "src/");
        assert_eq!(span.style.fg, Some(rgb_to_color(theme().status_added)));
    }

    #[test]
    fn dir_with_mixed_staged_and_unstaged_children_is_yellow() {
        let a = fd("src/a.rs");
        let b = fd("src/b.rs");
        let files = vec![
            staged_file(
                &a,
                StageStatus {
                    staged: Some(ChangeKind::Modified),
                    unstaged: None,
                },
            ),
            staged_file(
                &b,
                StageStatus {
                    staged: None,
                    unstaged: Some(ChangeKind::Modified),
                },
            ),
        ];
        let sidebar = Sidebar::build_with_icons(&files, &theme(), IconMode::Off);
        let span = dir_label_span(&sidebar.rows()[0], "src/");
        assert_eq!(span.style.fg, Some(rgb_to_color(theme().status_partial)));
    }

    #[test]
    fn dir_with_only_unstaged_children_is_default_not_muted() {
        let a = fd("src/a.rs");
        let files = vec![staged_file(
            &a,
            StageStatus {
                staged: None,
                unstaged: Some(ChangeKind::Untracked),
            },
        )];
        let sidebar = Sidebar::build_with_icons(&files, &theme(), IconMode::Off);
        let span = dir_label_span(&sidebar.rows()[0], "src/");
        // Default text: no explicit fg, no bold.
        assert_eq!(span.style.fg, None);
        assert!(!span.style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn dir_without_stage_data_keeps_muted_bold_fallback() {
        let a = fd("src/a.rs");
        let files = vec![SidebarFile {
            file: &a,
            added: 0,
            deleted: 0,
            stage: None,
        }];
        let sidebar = Sidebar::build_with_icons(&files, &theme(), IconMode::Off);
        let span = dir_label_span(&sidebar.rows()[0], "src/");
        assert_eq!(span.style.fg, Some(rgb_to_color(theme().muted)));
        assert!(span.style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn named_dir_uses_its_icon() {
        // A file under bin/ → the bin/ dir row's icon span carries the
        // `bin` glyph in lazygit's colour, not the generic folder glyph.
        let f = fd("bin/tool.rs");
        let files = vec![SidebarFile {
            file: &f,
            added: 0,
            deleted: 0,
            stage: None,
        }];
        let sidebar = Sidebar::build_with_icons(&files, &theme(), IconMode::On);
        let row = &sidebar.rows()[0];
        let icon_span = row
            .spans
            .iter()
            .find(|s| s.content.contains('\u{f12a7}'))
            .expect("bin icon span");
        assert_eq!(icon_span.style.fg, Some(rgb_to_color((0x25, 0xa7, 0x9a))));
    }

    #[test]
    fn unnamed_dir_keeps_folder_glyph() {
        // A file under foo/ (no named match) → the foo/ dir row uses the
        // generic folder glyph in the muted grey colour.
        let f = fd("foo/x.rs");
        let files = vec![SidebarFile {
            file: &f,
            added: 0,
            deleted: 0,
            stage: None,
        }];
        let sidebar = Sidebar::build_with_icons(&files, &theme(), IconMode::On);
        let row = &sidebar.rows()[0];
        let icon_span = row
            .spans
            .iter()
            .find(|s| s.content.contains('\u{f07b}'))
            .expect("folder glyph span");
        assert_eq!(icon_span.style.fg, Some(rgb_to_color((0x87, 0x87, 0x87))));
    }

    #[test]
    fn collapsed_chain_uses_deepest_segment_icon() {
        // src/bin/x.rs collapses to `src/bin/`; the row is looked up by
        // its deepest segment (`bin`, named), not the first (`src`,
        // unnamed).
        let f = fd("src/bin/x.rs");
        let files = vec![SidebarFile {
            file: &f,
            added: 0,
            deleted: 0,
            stage: None,
        }];
        let sidebar = Sidebar::build_with_icons(&files, &theme(), IconMode::On);
        let row = &sidebar.rows()[0];
        assert!(
            line_text(row).contains("src/bin/"),
            "expected collapsed chain label"
        );
        let icon_span = row
            .spans
            .iter()
            .find(|s| s.content.contains('\u{f12a7}'))
            .expect("bin icon span");
        assert_eq!(icon_span.style.fg, Some(rgb_to_color((0x25, 0xa7, 0x9a))));
    }

    #[test]
    fn nested_dir_aggregates_over_full_subtree() {
        // crates/deltoids/src/{lib.rs,parse.rs}: lib.rs staged-only,
        // parse.rs unstaged-only. The collapsed `crates/deltoids/src/`
        // header sees both → yellow.
        let a = fd("crates/deltoids/src/lib.rs");
        let b = fd("crates/deltoids/src/parse.rs");
        let files = vec![
            staged_file(
                &a,
                StageStatus {
                    staged: Some(ChangeKind::Modified),
                    unstaged: None,
                },
            ),
            staged_file(
                &b,
                StageStatus {
                    staged: None,
                    unstaged: Some(ChangeKind::Modified),
                },
            ),
        ];
        let sidebar = Sidebar::build_with_icons(&files, &theme(), IconMode::Off);
        let span = dir_label_span(&sidebar.rows()[0], "crates/deltoids/src/");
        assert_eq!(span.style.fg, Some(rgb_to_color(theme().status_partial)));
    }
}

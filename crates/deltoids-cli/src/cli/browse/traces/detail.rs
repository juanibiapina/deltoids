//! Detail/diff pane slice: the cached rendered diff, the header/wrapping
//! renderers, and the pane render itself. The diff body is rendered by the
//! shared [`deltoids::render_tui::render_hunk_list`], the same helper the
//! Files TUI uses.

use std::collections::HashMap;

use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};

use deltoids::render_tui::{
    pane_block_with_footer, pane_border_color, render_hunk_rows, render_pane_scrollbar,
    rgb_to_color,
};
use deltoids::{ChangeLayout, Theme};

use crate::HistoryEntry;
use crate::cli::browse::mode::{DrawBudget, layout_label, should_build_body};
use crate::cli::browse::text::wrap_text;

use crate::cli::browse::comment_view::{highlight_row, with_comments};
use crate::cli::browse::comments::{CommentAnchor, CommentScope, hunk_lines};
use crate::cli::browse::diff_cursor::{DiffRow, LinePlace, restore_cursor};

use super::model::LoadedTrace;
use super::{AppState, Focus};

/// Retained store of rendered entry diffs, keyed by
/// `(trace_index, entry_index)`. Every retained entry shares one `epoch`
/// (render width plus change layout); a change to either clears the store
/// (mirroring Files mode's `cached_width` rebuild). Retaining rendered
/// entries makes revisiting an entry instant instead of re-highlighting it
/// on every selection change.
#[derive(Debug, Default, Clone)]
pub(super) struct DiffCache {
    epoch: CacheEpoch,
    rows: HashMap<(usize, usize), Vec<DiffRow>>,
}

/// The identity every retained entry shares: its render `width`, the
/// active change `layout`, and the selected `syntax_theme` (a `&'static`
/// registry name). A change to any of them invalidates the whole store,
/// since each alters every entry's rendered rows.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) struct CacheEpoch {
    pub(super) width: usize,
    pub(super) layout: ChangeLayout,
    pub(super) syntax_theme: &'static str,
}

impl DiffCache {
    /// Rendered rows for `(trace, entry)` at `epoch`, or `None` on an
    /// epoch mismatch or a miss.
    pub(super) fn get(&self, epoch: CacheEpoch, key: (usize, usize)) -> Option<&Vec<DiffRow>> {
        if self.epoch != epoch {
            return None;
        }
        self.rows.get(&key)
    }

    /// Whether `(trace, entry)` is already rendered at `epoch`.
    pub(super) fn contains(&self, epoch: CacheEpoch, key: (usize, usize)) -> bool {
        self.epoch == epoch && self.rows.contains_key(&key)
    }

    /// Store rendered `rows` for `(trace, entry)`. A width or layout change
    /// clears the store first so every retained entry shares one epoch.
    pub(super) fn insert(&mut self, epoch: CacheEpoch, key: (usize, usize), rows: Vec<DiffRow>) {
        if self.epoch != epoch {
            self.rows.clear();
            self.epoch = epoch;
        }
        self.rows.insert(key, rows);
    }

    /// Drop all retained entries (used on reload, when disk data changed).
    pub(super) fn clear(&mut self) {
        self.rows.clear();
    }

    /// Whether the store holds no retained entries.
    #[cfg(test)]
    pub(super) fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

pub(super) fn max_detail_scroll(detail_row_count: usize, detail_height: usize) -> usize {
    detail_row_count.saturating_sub(detail_height.max(1))
}

/// Make sure `(trace, entry)` is rendered at `epoch`, splice this entry's
/// comments over it, and put the cursor back on its diff line. Rebuilding
/// the retained render happens only on a miss (a new entry, width, or
/// layout) — never for a comment, which is drawn as an overlay.
pub(super) fn ensure_diff_cache(
    active_trace: &LoadedTrace,
    state: &mut AppState,
    epoch: CacheEpoch,
    key: (usize, usize),
    theme: &Theme,
) {
    if !state.diff_cache.contains(epoch, key) {
        let rows = build_diff_rows(active_trace, key.1, epoch.width, epoch.layout, theme);
        state.diff_cache.insert(epoch, key, rows);
    }
    let AppState {
        diff_cache,
        comments,
        window,
        cursor,
        ..
    } = state;
    let Some(rows) = diff_cache.get(epoch, key) else {
        return;
    };
    // A recorded entry cannot change under the reviewer, so its comments
    // are never outdated.
    *window = with_comments(rows, comments, epoch.width, theme, |_, _| false);
    restore_cursor(window, cursor);
}

pub(super) fn render_diff_pane(
    frame: &mut ratatui::Frame<'_>,
    area: ratatui::layout::Rect,
    active_trace: &LoadedTrace,
    state: &mut AppState,
    layout: ChangeLayout,
    theme: &Theme,
    budget: DrawBudget,
) {
    let detail_width = area.width.saturating_sub(2) as usize;
    let epoch = CacheEpoch {
        width: detail_width,
        layout,
        syntax_theme: deltoids::theme_name_key(&theme.syntax_theme_name),
    };
    let key = (state.trace_index, state.entry_index());

    if should_build_body(budget, state.diff_cache.contains(epoch, key)) {
        ensure_diff_cache(active_trace, state, epoch, key, theme);
    }

    let diff_viewport = area.height.saturating_sub(2) as usize;
    // Transient status (e.g. copy feedback) takes over the footer; the
    // default footer shows the change layout and the help hint.
    let footer = match state.status.clone() {
        Some(status) => format!(" {status} "),
        None => format!(" {}  \u{00b7}  ? help ", layout_label(layout)),
    };
    let block = pane_block_with_footer(
        "─[3]─Diff─",
        pane_border_color(state.focus == Focus::Diff, theme),
        Some(footer),
    );

    match (!state.window.is_empty()).then_some(&state.window) {
        Some(rows) => {
            let detail_row_count = rows.len();
            let start = state.cursor.scroll.min(detail_row_count);
            let end = start
                .saturating_add(diff_viewport.max(1))
                .min(detail_row_count);
            let cursor_bg = rgb_to_color(theme.selection_bg);
            let show_cursor = state.focus == Focus::Diff;
            let visible_lines: Vec<Line<'static>> = rows[start..end]
                .iter()
                .enumerate()
                .map(|(offset, row)| {
                    if show_cursor && start + offset == state.cursor.row && row.is_selectable() {
                        highlight_row(row.line.clone(), detail_width, cursor_bg)
                    } else {
                        row.line.clone()
                    }
                })
                .collect();
            frame.render_widget(Paragraph::new(visible_lines).block(block), area);
            render_pane_scrollbar(
                frame,
                area,
                detail_row_count,
                state.cursor.scroll,
                diff_viewport,
                state.focus == Focus::Diff,
                theme,
            );
        }
        None => {
            // Fast frame, entry not yet rendered: show the cheap header
            // plus a muted placeholder. The body fills in on the settling
            // Full frame and is then retained.
            let placeholder = render_detail_placeholder(active_trace, key.1, detail_width, theme);
            let visible: Vec<Line<'static>> =
                placeholder.into_iter().take(diff_viewport.max(1)).collect();
            frame.render_widget(Paragraph::new(visible).block(block), area);
        }
    }
}

/// Cheap stand-in for a not-yet-rendered entry: the detail header (fast to
/// build) plus a muted "Rendering…" line, so fast scrolling still shows
/// each entry's reason/path/metadata without the per-line syntax cost.
fn render_detail_placeholder(
    trace: &LoadedTrace,
    entry_index: usize,
    width: usize,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let Some(entry) = trace.entries.get(entry_index) else {
        return Vec::new();
    };
    let mut lines = render_detail_header(entry, width, theme);
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "Rendering…".to_string(),
        Style::default().fg(rgb_to_color(theme.muted)),
    )));
    lines
}

/// Display `path` relative to the entry's `cwd`.
///
/// Traces are filtered to the current directory, so `cwd` is where the
/// TUI is running; stripping it yields the path the user typed. Falls
/// back to collapsing `$HOME` to `~` when the path is not under `cwd`
/// (e.g. an absolute path outside the tree).
pub(super) fn display_path(path: &str, cwd: &str) -> String {
    // Only treat as relative on a real path boundary, so a sibling that
    // merely shares the prefix (`/a/project-2` vs `/a/project`) is not
    // mistaken for a child.
    if !cwd.is_empty()
        && let Some(rest) = path.strip_prefix(cwd)
        && let Some(rel) = rest.strip_prefix('/')
        && !rel.is_empty()
    {
        return rel.to_string();
    }
    collapse_home(path)
}

fn collapse_home(path: &str) -> String {
    let Some(home) = std::env::var_os("HOME") else {
        return path.to_string();
    };
    let home = home.to_string_lossy();
    if home.is_empty() {
        return path.to_string();
    }
    if let Some(rest) = path.strip_prefix(home.as_ref()) {
        if rest.is_empty() {
            return "~".to_string();
        }
        if rest.starts_with('/') {
            return format!("~{rest}");
        }
    }
    path.to_string()
}

/// Build the diff pane's rows for one entry: the detail header, then each
/// hunk, with every logical diff line anchored to a [`CommentAnchor`] and
/// any saved comment drawn under the line's last wrapped row.
pub(super) fn build_diff_rows(
    trace: &LoadedTrace,
    entry_index: usize,
    width: usize,
    layout: ChangeLayout,
    theme: &Theme,
) -> Vec<DiffRow> {
    let Some(entry) = trace.entries.get(entry_index) else {
        return Vec::new();
    };

    let mut rendered: Vec<DiffRow> = render_detail_header(entry, width, theme)
        .into_iter()
        .map(DiffRow::plain)
        .collect();

    if !entry.ok {
        if let Some(err) = entry.error.as_deref() {
            rendered.push(DiffRow::plain(Line::from("")));
            rendered.push(DiffRow::plain(labeled_line("error", err, Color::Red)));
        }
    } else if entry.hunks.is_empty() {
        // v1 entries have no hunks; show deprecation notice.
        rendered.push(DiffRow::plain(Line::from("")));
        rendered.push(DiffRow::plain(Line::from("(old format, cannot display)")));
    } else {
        for (hunk_index, hunk) in entry.hunks.iter().enumerate() {
            // Blank separator before each hunk, matching render_hunk_list.
            rendered.push(DiffRow::plain(Line::from("")));
            push_hunk_rows(
                &mut rendered,
                hunk,
                entry,
                hunk_index,
                &scope(trace, entry_index),
                width,
                layout,
                theme,
            );
        }
    }

    rendered
}

/// The comment scope for one entry of `trace`.
pub(super) fn scope(trace: &LoadedTrace, entry_index: usize) -> CommentScope {
    CommentScope::TraceEntry {
        trace_id: trace.trace.trace_id.clone(),
        entry_index,
    }
}

/// Render one hunk into `rows`, tagging each row with the diff line it
/// renders. The entry path, hunk position, and line index give every diff
/// line a stable identity for the cursor.
#[allow(clippy::too_many_arguments)]
fn push_hunk_rows(
    rows: &mut Vec<DiffRow>,
    hunk: &deltoids::Hunk,
    entry: &HistoryEntry,
    hunk_index: usize,
    scope: &CommentScope,
    width: usize,
    layout: ChangeLayout,
    theme: &Theme,
) {
    let anchors: Vec<CommentAnchor> = hunk_lines(hunk)
        .map(|line| CommentAnchor {
            scope: scope.clone(),
            path: entry.path.clone(),
            side: line.side,
            line: line.number,
        })
        .collect();

    for row in render_hunk_rows(hunk, entry.highlight.as_deref(), width, layout, theme) {
        let Some(index) = row.source_line else {
            rows.push(DiffRow::plain(row.line));
            continue;
        };
        rows.push(DiffRow::line_row(
            row.line,
            anchors[index].clone(),
            row.first_row.then(|| LinePlace {
                file: entry.path.clone(),
                hunk: hunk_index,
                index,
            }),
            row.last_row,
        ));
    }
}

fn render_detail_header(entry: &HistoryEntry, width: usize, theme: &Theme) -> Vec<Line<'static>> {
    let path = display_path(&entry.path, &entry.cwd);
    render_header_block(&entry.reason, &path, width, theme)
}

fn labeled_line(label: &str, value: &str, color: Color) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("{label}: "),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::raw(value.to_string()),
    ])
}

fn render_header_block(
    reason: &str,
    path: &str,
    width: usize,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let reason_style = Style::default()
        .fg(rgb_to_color(theme.border_active))
        .add_modifier(Modifier::BOLD);

    if width < 4 {
        return vec![Line::from(Span::styled(
            fit_line(reason, width),
            reason_style,
        ))];
    }

    // Terminal default foreground (white on dark), bold, matching the
    // Files tab's file header.
    let path_style = Style::default()
        .fg(Color::Reset)
        .add_modifier(Modifier::BOLD);
    let border = Style::default().fg(rgb_to_color(theme.border));
    let bot = format!("─{}", "─".repeat(width.saturating_sub(1)));

    let mut lines = Vec::new();
    for wrapped in wrap_text(reason, width) {
        lines.push(Line::from(Span::styled(wrapped, reason_style)));
    }
    lines.push(Line::from(Span::styled(fit_line(path, width), path_style)));
    lines.push(Line::from(Span::styled(bot, border)));
    lines
}

pub(super) fn fit_line(line: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }

    let mut result = String::new();
    for ch in line.chars().take(width) {
        result.push(ch);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::browse::traces::test_support::*;

    fn grouped_epoch(width: usize) -> CacheEpoch {
        CacheEpoch {
            width,
            layout: ChangeLayout::Grouped,
            syntax_theme: "",
        }
    }

    #[test]
    fn diff_cache_retains_entries_across_selection_changes() {
        let mut cache = DiffCache::default();
        let e = grouped_epoch(80);
        cache.insert(e, (0, 0), vec![DiffRow::plain(Line::from("a"))]);
        cache.insert(
            e,
            (0, 1),
            vec![
                DiffRow::plain(Line::from("b")),
                DiffRow::plain(Line::from("b2")),
            ],
        );

        // Both entries stay retained: revisiting the first is a cache hit.
        assert!(cache.contains(e, (0, 0)));
        assert!(cache.contains(e, (0, 1)));
        assert_eq!(cache.get(e, (0, 0)).map(|rows| rows.len()), Some(1));
        assert_eq!(cache.get(e, (0, 1)).map(|rows| rows.len()), Some(2));
    }

    #[test]
    fn diff_cache_width_change_clears_store() {
        let mut cache = DiffCache::default();
        cache.insert(
            grouped_epoch(80),
            (0, 0),
            vec![DiffRow::plain(Line::from("a"))],
        );
        assert!(cache.contains(grouped_epoch(80), (0, 0)));

        // A different width drops the stale entry and rebuilds at the new width.
        assert!(!cache.contains(grouped_epoch(79), (0, 0)));
        assert!(cache.get(grouped_epoch(79), (0, 0)).is_none());
        cache.insert(
            grouped_epoch(79),
            (0, 1),
            vec![DiffRow::plain(Line::from("b"))],
        );
        assert!(!cache.contains(grouped_epoch(79), (0, 0)));
        assert!(cache.contains(grouped_epoch(79), (0, 1)));
    }

    #[test]
    fn diff_cache_layout_change_clears_store() {
        let mut cache = DiffCache::default();
        cache.insert(
            grouped_epoch(80),
            (0, 0),
            vec![DiffRow::plain(Line::from("a"))],
        );
        assert!(cache.contains(grouped_epoch(80), (0, 0)));

        // Same width, different layout: the stale entry is dropped.
        let interleaved = CacheEpoch {
            width: 80,
            layout: ChangeLayout::Interleaved {
                group: std::num::NonZeroUsize::new(1).unwrap(),
            },
            syntax_theme: "",
        };
        assert!(!cache.contains(interleaved, (0, 0)));
        cache.insert(interleaved, (0, 1), vec![DiffRow::plain(Line::from("b"))]);
        assert!(!cache.contains(interleaved, (0, 0)));
        assert!(cache.contains(interleaved, (0, 1)));
    }

    #[test]
    fn ensure_diff_cache_builds_once_then_hits() {
        use crate::cli::browse::traces::test_support::*;
        let trace = LoadedTrace {
            trace: trace_summary("01JTESTTRACE00000000000000", 1, "a"),
            entries: vec![edit_entry()],
        };
        let theme = test_theme();
        let mut state = AppState::new(1);
        let e = grouped_epoch(80);

        assert!(!state.diff_cache.contains(e, (0, 0)));
        ensure_diff_cache(&trace, &mut state, e, (0, 0), &theme);
        assert!(state.diff_cache.contains(e, (0, 0)));
        // Second call is a no-op hit; the store still holds the one entry.
        ensure_diff_cache(&trace, &mut state, e, (0, 0), &theme);
        assert!(state.diff_cache.contains(e, (0, 0)));
    }

    /// Concatenate the visible text of a `Line<'static>` (ignoring styles).
    fn line_text(line: &Line<'static>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    fn render_entry(entry: &HistoryEntry, width: usize, theme: &Theme) -> Vec<Line<'static>> {
        let trace = LoadedTrace {
            trace: trace_summary("01JTESTTRACE00000000000000", 1, "a"),
            entries: vec![entry.clone()],
        };
        build_diff_rows(&trace, 0, width, ChangeLayout::Grouped, theme)
            .into_iter()
            .map(|row| row.line)
            .collect()
    }

    #[test]
    fn render_detail_for_text_single_edit_shows_pure_hunk_body() {
        use deltoids::{DiffLine, Hunk, LineKind, ScopeNode};

        // A single text edit whose per-edit reason mirrors the top-level
        // reason: the header shows the reason and the body is a pure hunk,
        // no edit box.
        let theme = test_theme();
        let mut entry = edit_entry();
        entry.edits[0].reason = entry.reason.clone();
        entry.hunks = vec![Hunk {
            old_start: 5,
            new_start: 5,
            lines: vec![
                DiffLine {
                    kind: LineKind::Context,
                    content: "context line".to_string(),
                },
                DiffLine {
                    kind: LineKind::Removed,
                    content: "old line".to_string(),
                },
                DiffLine {
                    kind: LineKind::Added,
                    content: "new line".to_string(),
                },
            ],
            ancestors: vec![ScopeNode {
                kind: "function_item".to_string(),
                name: "my_func".to_string(),
                start_line: 3,
                end_line: 10,
                text: "fn my_func() {".to_string(),
            }],
        }];

        let lines = render_entry(&entry, 80, &theme);
        let header = render_detail_header(&entry, 80, &theme);
        let body = deltoids::render_tui::render_hunk_list(
            &entry.hunks,
            None,
            80,
            ChangeLayout::Grouped,
            &theme,
        );

        // Header first, then exactly the shared hunk body (no edit box in
        // between).
        assert_eq!(lines.len(), header.len() + body.len());
        let texts: Vec<String> = lines.iter().map(line_text).collect();
        assert!(texts.iter().any(|t| t.starts_with("Update x constant")));
        assert!(texts.iter().any(|t| t.contains("new line")));
    }

    #[test]
    fn render_detail_for_legacy_multi_edit_shows_only_top_level_reason() {
        use crate::TextEdit;
        use deltoids::{DiffLine, Hunk, LineKind};

        // An old multi-edit trace entry (two edits, two hunks) renders with
        // the top-level reason in the header and a pure hunk body; the
        // distinct per-edit reasons are not shown anywhere.
        let theme = test_theme();
        let mut entry = edit_entry();
        entry.edits = vec![
            TextEdit {
                reason: "First edit".to_string(),
                old_text: "a".to_string(),
                new_text: "A".to_string(),
            },
            TextEdit {
                reason: "Second edit".to_string(),
                old_text: "b".to_string(),
                new_text: "B".to_string(),
            },
        ];
        let hunk = |content: &str| Hunk {
            old_start: 1,
            new_start: 1,
            lines: vec![DiffLine {
                kind: LineKind::Added,
                content: content.to_string(),
            }],
            ancestors: Vec::new(),
        };
        entry.hunks = vec![hunk("A"), hunk("B")];

        let lines = render_entry(&entry, 80, &theme);
        let texts: Vec<String> = lines.iter().map(line_text).collect();

        assert!(texts.iter().any(|t| t.starts_with("Update x constant")));
        // Per-edit reasons are gone from the body.
        assert!(!texts.iter().any(|t| t.contains("First edit")));
        assert!(!texts.iter().any(|t| t.contains("Second edit")));
        // Both hunks still render.
        assert!(texts.iter().any(|t| t.contains('A')));
        assert!(texts.iter().any(|t| t.contains('B')));
    }

    #[test]
    fn render_detail_for_v1_entry_shows_old_format_notice() {
        let theme = test_theme();
        let entry = edit_entry(); // v1 entry with empty hunks
        let lines = render_entry(&entry, 80, &theme);
        let texts: Vec<String> = lines.iter().map(line_text).collect();
        assert!(
            texts
                .iter()
                .any(|t| t.contains("old format, cannot display"))
        );
    }

    #[test]
    fn render_detail_for_error_entry_shows_error_line() {
        let theme = test_theme();
        let mut entry = edit_entry();
        entry.ok = false;
        entry.error = Some("something failed".to_string());
        let lines = render_entry(&entry, 80, &theme);
        let texts: Vec<String> = lines.iter().map(line_text).collect();
        assert!(texts.iter().any(|t| t.contains("something failed")));
    }

    #[test]
    fn collapse_home_handles_home_prefix() {
        // SAFETY: single-threaded test module and HOME is only read via
        // collapse_home here.
        unsafe { std::env::set_var("HOME", "/home/alice") };
        assert_eq!(
            collapse_home("/home/alice/project/app.rs"),
            "~/project/app.rs"
        );
        assert_eq!(collapse_home("/home/alice"), "~");
        assert_eq!(
            collapse_home("/home/alice-extra/app.rs"),
            "/home/alice-extra/app.rs"
        );
        assert_eq!(collapse_home("/other/path"), "/other/path");
    }

    #[test]
    fn display_path_strips_cwd_prefix() {
        assert_eq!(
            display_path("/tmp/project/app.txt", "/tmp/project"),
            "app.txt"
        );
        assert_eq!(
            display_path("/tmp/project/src/main.rs", "/tmp/project"),
            "src/main.rs"
        );
    }

    #[test]
    fn display_path_falls_back_to_home_outside_cwd() {
        // SAFETY: single-threaded test module; HOME is only read here.
        unsafe { std::env::set_var("HOME", "/home/alice") };
        // Not under cwd: collapse HOME instead.
        assert_eq!(
            display_path("/home/alice/other/x.rs", "/tmp/project"),
            "~/other/x.rs"
        );
        // A sibling that merely shares a prefix is not treated as relative.
        assert_eq!(
            display_path("/tmp/project-2/x.rs", "/tmp/project"),
            "/tmp/project-2/x.rs"
        );
    }

    #[test]
    fn render_detail_header_uses_reason_path_and_rule() {
        let theme = test_theme();
        let lines = render_detail_header(&edit_entry(), 80, &theme);
        // Header is reason, path, bottom rule; no tool/status metadata line.
        assert_eq!(lines.len(), 3);
        assert!(lines[0].to_string().starts_with("Update x constant"));
        assert_eq!(
            lines[0].spans[0].style.fg,
            Some(rgb_to_color(theme.border_active))
        );
        // Path shown relative to the entry cwd (/tmp/project), in the
        // terminal default foreground, bold.
        assert!(lines[1].to_string().starts_with("app.txt"));
        assert_eq!(lines[1].spans[0].style.fg, Some(Color::Reset));
        assert!(
            lines[1].spans[0]
                .style
                .add_modifier
                .contains(Modifier::BOLD)
        );
        let bottom = lines[2].to_string();
        assert!(bottom.starts_with('─'));
        assert!(!bottom.contains('╯'), "bottom rule should have no corner");
        assert!(!bottom.contains('│'), "no right border");
    }

    #[test]
    fn render_detail_header_wraps_long_reason() {
        let theme = test_theme();
        let mut entry = edit_entry();
        entry.reason = "This is a long reason that should wrap onto multiple lines".to_string();
        let lines = render_detail_header(&entry, 30, &theme);
        // Reason wraps into multiple lines, then path, rule.
        assert!(
            lines.len() > 3,
            "long reason should produce more than 3 lines, got {}",
            lines.len()
        );
        // All reason lines are border_active (orange) bold.
        let rule_index = lines
            .iter()
            .position(|l| l.to_string().starts_with('─'))
            .expect("should have a bottom rule");
        // Lines before the path line (rule_index - 1) are the reason.
        for line in &lines[..rule_index - 1] {
            assert_eq!(
                line.spans[0].style.fg,
                Some(rgb_to_color(theme.border_active)),
                "wrapped reason line should be border_active color"
            );
        }
        // No right border on any line.
        for line in &lines {
            assert!(
                !line.to_string().contains('│'),
                "no line should have right border"
            );
        }
    }

    #[test]
    fn render_detail_header_falls_back_cleanly_when_narrow() {
        let theme = test_theme();
        let lines = render_detail_header(&edit_entry(), 3, &theme);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].to_string(), "Upd");
    }
}

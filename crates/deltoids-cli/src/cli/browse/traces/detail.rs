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
use unicode_width::UnicodeWidthChar;

use deltoids::Theme;
use deltoids::render_tui::{
    self, pane_block, pane_border_color, render_pane_scrollbar, rgb_to_color,
};

use crate::HistoryEntry;
use crate::cli::browse::mode::{DrawBudget, should_build_body};

use super::model::LoadedTrace;
use super::{AppState, Focus};

/// Retained store of rendered entry diffs, keyed by
/// `(trace_index, entry_index)`. Every retained entry shares one `width`;
/// a width change clears the store (mirroring Files mode's `cached_width`
/// rebuild). Retaining rendered entries makes revisiting an entry instant
/// instead of re-highlighting it on every selection change.
#[derive(Debug, Default, Clone)]
pub(super) struct DiffCache {
    width: usize,
    lines: HashMap<(usize, usize), Vec<Line<'static>>>,
}

impl DiffCache {
    /// Rendered lines for `(trace, entry)` at `width`, or `None` on a
    /// width mismatch or a miss.
    fn get(&self, width: usize, key: (usize, usize)) -> Option<&Vec<Line<'static>>> {
        if self.width != width {
            return None;
        }
        self.lines.get(&key)
    }

    /// Whether `(trace, entry)` is already rendered at `width`.
    fn contains(&self, width: usize, key: (usize, usize)) -> bool {
        self.width == width && self.lines.contains_key(&key)
    }

    /// Store rendered `lines` for `(trace, entry)`. A width change clears
    /// the store first so every retained entry shares one width.
    pub(super) fn insert(&mut self, width: usize, key: (usize, usize), lines: Vec<Line<'static>>) {
        if self.width != width {
            self.lines.clear();
            self.width = width;
        }
        self.lines.insert(key, lines);
    }

    /// Drop all retained entries (used on reload, when disk data changed).
    pub(super) fn clear(&mut self) {
        self.lines.clear();
    }

    /// Whether the store holds no retained entries.
    #[cfg(test)]
    pub(super) fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// Row count for `(trace, entry)` at the store's current width (0 on a
    /// miss). Used for scroll math between draws.
    pub(super) fn rows_for(&self, key: (usize, usize)) -> usize {
        self.lines.get(&key).map(|lines| lines.len()).unwrap_or(0)
    }
}

pub(super) fn max_detail_scroll(detail_row_count: usize, detail_height: usize) -> usize {
    detail_row_count.saturating_sub(detail_height.max(1))
}

fn ensure_diff_cache(
    active_trace: &LoadedTrace,
    state: &mut AppState,
    detail_width: usize,
    key: (usize, usize),
    theme: &Theme,
) {
    if state.diff_cache.contains(detail_width, key) {
        return;
    }
    let lines = render_detail_for(active_trace, key.1, detail_width, theme);
    state.diff_cache.insert(detail_width, key, lines);
}

pub(super) fn render_diff_pane(
    frame: &mut ratatui::Frame<'_>,
    area: ratatui::layout::Rect,
    active_trace: &LoadedTrace,
    state: &mut AppState,
    theme: &Theme,
    budget: DrawBudget,
) {
    let detail_width = area.width.saturating_sub(2) as usize;
    let key = (state.trace_index, state.entry_index());

    if should_build_body(budget, state.diff_cache.contains(detail_width, key)) {
        ensure_diff_cache(active_trace, state, detail_width, key, theme);
    }

    let diff_viewport = area.height.saturating_sub(2) as usize;
    let block = pane_block(
        "─[3]─Diff─",
        pane_border_color(state.focus == Focus::Diff, theme),
    );

    match state.diff_cache.get(detail_width, key) {
        Some(lines) => {
            let detail_row_count = lines.len();
            let start = state.diff_scroll.min(detail_row_count);
            let end = start
                .saturating_add(diff_viewport.max(1))
                .min(detail_row_count);
            let visible_lines: Vec<Line<'static>> = lines[start..end].to_vec();
            frame.render_widget(Paragraph::new(visible_lines).block(block), area);
            render_pane_scrollbar(
                frame,
                area,
                detail_row_count,
                state.diff_scroll,
                diff_viewport,
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
fn display_path(path: &str, cwd: &str) -> String {
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

pub(super) fn render_detail_for(
    trace: &LoadedTrace,
    entry_index: usize,
    width: usize,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let Some(entry) = trace.entries.get(entry_index) else {
        return Vec::new();
    };

    let mut rendered = render_detail_header(entry, width, theme);

    if !entry.ok {
        if let Some(err) = entry.error.as_deref() {
            rendered.push(Line::from(""));
            rendered.push(labeled_line("error", err, Color::Red));
        }
    } else if entry.hunks.is_empty() {
        // v1 entries have no hunks; show deprecation notice.
        rendered.push(Line::from(""));
        rendered.push(Line::from("(old format, cannot display)"));
    } else {
        // The hunk body carries its own leading blank separator, so the
        // header/body gap comes from render_hunk_list.
        rendered.extend(render_tui::render_hunk_list(
            &entry.hunks,
            entry.highlight.as_deref(),
            width,
            theme,
        ));
    }

    rendered
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

fn display_width(text: &str) -> usize {
    text.chars()
        .map(|ch| {
            if ch == '\t' {
                4
            } else {
                ch.width().unwrap_or(0)
            }
        })
        .sum()
}

fn split_word_to_width(word: &str, max_width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut chunk = String::new();
    let mut chunk_width = 0usize;

    for ch in word.chars() {
        let ch_width = if ch == '\t' {
            4
        } else {
            ch.width().unwrap_or(0)
        };
        if chunk_width + ch_width > max_width && !chunk.is_empty() {
            lines.push(chunk);
            chunk = String::new();
            chunk_width = 0;
        }
        chunk.push(ch);
        chunk_width += ch_width;
    }

    if !chunk.is_empty() {
        lines.push(chunk);
    }

    lines
}

fn wrap_text(text: &str, max_width: usize) -> Vec<String> {
    if max_width == 0 {
        return vec![String::new()];
    }

    let mut lines = Vec::new();
    let mut current = String::new();
    let mut current_width = 0usize;

    for word in text.split_whitespace() {
        let word_width = display_width(word);

        if word_width > max_width {
            // Word too long for a single line: flush current, then split by character.
            if !current.is_empty() {
                lines.push(current);
                current = String::new();
                current_width = 0;
            }
            let mut chunks = split_word_to_width(word, max_width).into_iter();
            if let Some(last) = chunks.next_back() {
                lines.extend(chunks);
                current_width = display_width(&last);
                current = last;
            }
            continue;
        }

        if current.is_empty() {
            current = word.to_string();
            current_width = word_width;
        } else if current_width + 1 + word_width <= max_width {
            current.push(' ');
            current.push_str(word);
            current_width += 1 + word_width;
        } else {
            lines.push(current);
            current = word.to_string();
            current_width = word_width;
        }
    }

    if !current.is_empty() {
        lines.push(current);
    }

    if lines.is_empty() {
        lines.push(String::new());
    }

    lines
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

    #[test]
    fn diff_cache_retains_entries_across_selection_changes() {
        let mut cache = DiffCache::default();
        cache.insert(80, (0, 0), vec![Line::from("a")]);
        cache.insert(80, (0, 1), vec![Line::from("b"), Line::from("b2")]);

        // Both entries stay retained: revisiting the first is a cache hit.
        assert!(cache.contains(80, (0, 0)));
        assert!(cache.contains(80, (0, 1)));
        assert_eq!(cache.rows_for((0, 0)), 1);
        assert_eq!(cache.rows_for((0, 1)), 2);
        assert_eq!(cache.get(80, (0, 0)).map(|l| l.len()), Some(1));
    }

    #[test]
    fn diff_cache_width_change_clears_store() {
        let mut cache = DiffCache::default();
        cache.insert(80, (0, 0), vec![Line::from("a")]);
        assert!(cache.contains(80, (0, 0)));

        // A different width drops the stale entry and rebuilds at the new width.
        assert!(!cache.contains(79, (0, 0)));
        assert!(cache.get(79, (0, 0)).is_none());
        cache.insert(79, (0, 1), vec![Line::from("b")]);
        assert!(!cache.contains(79, (0, 0)));
        assert!(cache.contains(79, (0, 1)));
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

        assert!(!state.diff_cache.contains(80, (0, 0)));
        ensure_diff_cache(&trace, &mut state, 80, (0, 0), &theme);
        assert!(state.diff_cache.contains(80, (0, 0)));
        // Second call is a no-op hit; the store still holds the one entry.
        ensure_diff_cache(&trace, &mut state, 80, (0, 0), &theme);
        assert!(state.diff_cache.contains(80, (0, 0)));
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
        render_detail_for(&trace, 0, width, theme)
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
        let body = render_tui::render_hunk_list(&entry.hunks, None, 80, &theme);

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

    #[test]
    fn wrap_text_fits_on_one_line() {
        assert_eq!(wrap_text("short", 80), vec!["short"]);
    }

    #[test]
    fn wrap_text_wraps_at_word_boundary() {
        assert_eq!(wrap_text("hello world foo", 11), vec!["hello world", "foo"]);
    }

    #[test]
    fn wrap_text_splits_long_word_by_character() {
        assert_eq!(wrap_text("abcdefghij", 4), vec!["abcd", "efgh", "ij"]);
    }

    #[test]
    fn wrap_text_empty_string() {
        assert_eq!(wrap_text("", 80), vec![""]);
    }

    #[test]
    fn wrap_text_exact_fit() {
        assert_eq!(wrap_text("abcd", 4), vec!["abcd"]);
    }

    #[test]
    fn wrap_text_single_word_longer_than_width() {
        assert_eq!(wrap_text("abcdef", 4), vec!["abcd", "ef"]);
    }

    #[test]
    fn split_word_to_width_splits_by_display_width() {
        assert_eq!(
            split_word_to_width("abcdefghij", 4),
            vec!["abcd", "efgh", "ij"]
        );
    }

    #[test]
    fn wrap_text_mixed_short_and_long_words() {
        assert_eq!(
            wrap_text("hi abcdefgh there", 6),
            vec!["hi", "abcdef", "gh", "there"]
        );
    }
}

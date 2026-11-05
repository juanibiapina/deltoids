//! Sidebar pane vertical slice: builds the [`Sidebar`] model from a
//! [`Model`], handles its movement keys, and renders it (rows, selection
//! bar, footer). Selection-driven scrolling of the diff pane is
//! coordination owned by the shell, not this slice.

use crossterm::event::KeyCode;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use deltoids::Theme;
use deltoids::render_tui::{pane_border_color, rgb_to_color};

use crate::sidebar::{Sidebar, SidebarFile, display_path};

use super::model::{Model, body_deltas};
use super::pane;

/// Build the sidebar from a model plus per-file delta counts.
pub(super) fn build_sidebar(model: &Model, theme: &Theme) -> Sidebar {
    let mut sidebar = Sidebar::build(&sidebar_files(model), theme);
    mark_visited(&mut sidebar, model);
    sidebar
}

pub(super) fn rebuild_sidebar(sidebar: &Sidebar, model: &Model, hidden: &[bool]) -> Sidebar {
    let mut sidebar = sidebar.rebuilt(&sidebar_files(model), hidden);
    mark_visited(&mut sidebar, model);
    sidebar
}

/// Mark the files a jump opened, when there are any.
fn mark_visited(sidebar: &mut Sidebar, model: &Model) {
    if model.visited > 0 {
        let visited: Vec<bool> = (0..model.files.len())
            .map(|index| model.is_visited(index))
            .collect();
        sidebar.set_visited(&visited);
    }
}

pub(super) fn update_staging(sidebar: &mut Sidebar, model: &Model) {
    let stages: Vec<_> = model
        .files
        .iter()
        .map(|file| model.stages.get(display_path(&file.file)).copied())
        .collect();
    sidebar.update_staging(&stages);
}

fn sidebar_files(model: &Model) -> Vec<SidebarFile<'_>> {
    model
        .files
        .iter()
        .zip(model.bodies.iter())
        .map(|(f, b)| {
            let (added, deleted) = body_deltas(b);
            let stage = model.stages.get(display_path(&f.file)).copied();
            SidebarFile {
                file: &f.file,
                added,
                deleted,
                stage,
            }
        })
        .collect()
}

/// Handle a movement key while the sidebar is focused. Returns `true`
/// when the selection moved, so the shell can snap the diff pane to the
/// newly selected file (the cross-pane coordination it owns). Enter
/// folds or unfolds the selected directory; the selection stays put.
pub(super) fn handle_key(sidebar: &mut Sidebar, key: KeyCode, viewport: usize) -> bool {
    match key {
        KeyCode::Char('j') | KeyCode::Down => sidebar.move_down(viewport),
        KeyCode::Char('k') | KeyCode::Up => sidebar.move_up(viewport),
        KeyCode::PageDown => sidebar.page_down(viewport),
        KeyCode::PageUp => sidebar.page_up(viewport),
        KeyCode::Char('g') | KeyCode::Home => sidebar.top(viewport),
        KeyCode::Char('G') | KeyCode::End => sidebar.bottom(viewport),
        KeyCode::Enter => {
            sidebar.toggle_selected_dir(viewport);
            return false;
        }
        _ => return false,
    }
    true
}

pub(super) fn draw_sidebar(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    sidebar: &Sidebar,
    chrome: pane::Chrome,
    theme: &Theme,
) {
    let focused = chrome.focused;
    let inner = pane::draw(frame, area, chrome, theme);
    let rows = SidebarRows {
        body: inner,
        scrollbar_x: area.right().saturating_sub(1),
        highlight: true,
        focused,
    };
    draw_sidebar_rows(frame, rows, sidebar, theme);
}

/// Where the sidebar's rows draw: their body, the scrollbar's column,
/// whether the selected row is highlighted, and whether the pane has
/// focus.
pub(super) struct SidebarRows {
    pub(super) body: Rect,
    pub(super) scrollbar_x: u16,
    pub(super) highlight: bool,
    pub(super) focused: bool,
}

/// Draw the sidebar's rows into `rows.body`, without pane chrome.
pub(super) fn draw_sidebar_rows(
    frame: &mut ratatui::Frame<'_>,
    rows: SidebarRows,
    sidebar: &Sidebar,
    theme: &Theme,
) {
    let SidebarRows {
        body: inner,
        scrollbar_x,
        highlight,
        focused,
    } = rows;
    let viewport = inner.height as usize;
    let inner_width = inner.width as usize;
    let scroll = sidebar.scroll();
    let total = sidebar.row_count();
    let start = scroll.min(total);
    let end = start.saturating_add(viewport.max(1)).min(total);
    let selected_bg = Style::default().bg(rgb_to_color(theme.selection_bg));
    let mut visible: Vec<Line<'static>> = sidebar.rows()[start..end]
        .iter()
        .zip(&sidebar.row_notes()[start..end])
        .enumerate()
        .map(|(offset, (row, note))| {
            let selected = start + offset == sidebar.selected();
            let fill = if highlight && selected {
                selected_bg
            } else {
                Style::default()
            };
            if selected && !highlight {
                return with_note_at_right(&unselected(row), note.as_ref(), inner_width, fill);
            }
            with_note_at_right(row, note.as_ref(), inner_width, fill)
        })
        .collect();

    // Extend the selection background across the full inner pane width
    // so the highlighted row reads as a continuous bar (matching
    // lazygit's `List` widget). Pad against the inner
    // width so the trailing block stops just before the scrollbar.
    if let Some(rel) = sidebar.selected().checked_sub(scroll)
        && rel < visible.len()
        && highlight
    {
        pad_selected_row(&mut visible[rel], inner_width, theme);
    }

    frame.render_widget(Paragraph::new(visible), inner);
    if let Some(offset) = sidebar
        .rule_position()
        .and_then(|position| position.checked_sub(start))
        .filter(|&offset| start + offset < end)
    {
        let color = pane_border_color(focused, theme);
        draw_rule(frame, inner, inner.y + offset as u16, color);
    }

    pane::draw_scrollbar_beside(
        frame,
        inner,
        scrollbar_x,
        total,
        sidebar.selected(),
        focused,
        theme,
    );
}

/// The selected row drawn as any other: without the selection's
/// background and bold, for a list the cursor is not in.
fn unselected(row: &Line<'static>) -> Line<'static> {
    let mut row = row.clone();
    for span in &mut row.spans {
        span.style.bg = None;
        span.style
            .add_modifier
            .remove(ratatui::style::Modifier::BOLD);
    }
    row
}

/// A `───` rule across `body` at screen row `y`.
fn draw_rule(frame: &mut ratatui::Frame<'_>, body: Rect, y: u16, color: ratatui::style::Color) {
    let rule = "─".repeat(body.width as usize);
    let row = Rect {
        x: body.x,
        y,
        width: body.width,
        height: 1,
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(rule, Style::default().fg(color)))),
        row,
    );
}

/// A row too long for both is cut so the note stays whole.
fn with_note_at_right(
    row: &Line<'static>,
    note: Option<&Line<'static>>,
    width: usize,
    fill: Style,
) -> Line<'static> {
    let Some(note) = note else {
        return row.clone();
    };
    let note_width = note.width();
    let room = width.saturating_sub(note_width + 1);
    let mut spans = row.spans.clone();
    while spans_width(&spans) > room && spans.last().is_some_and(is_line_count) {
        spans.pop();
        while spans
            .last()
            .is_some_and(|span| span.content.trim().is_empty())
        {
            spans.pop();
        }
    }
    let mut spans = cut_to_width(&spans, room);
    let used: usize = spans.iter().map(|span| span.content.width()).sum();
    spans.push(Span::styled(
        " ".repeat(width.saturating_sub(used + note_width).max(1)),
        fill,
    ));
    spans.extend(note.spans.iter().cloned());
    Line::from(spans)
}

fn spans_width(spans: &[Span<'_>]) -> usize {
    spans.iter().map(|span| span.content.width()).sum()
}

/// `+N` and `-N` line counts give way before the file name is cut.
fn is_line_count(span: &Span<'_>) -> bool {
    let text = span.content.as_ref();
    text.len() > 1
        && (text.starts_with('+') || text.starts_with('-'))
        && text[1..].chars().all(|c| c.is_ascii_digit())
}

fn cut_to_width(spans: &[Span<'static>], width: usize) -> Vec<Span<'static>> {
    let mut out = Vec::new();
    let mut used = 0;
    for span in spans {
        let span_width = span.content.width();
        if used + span_width <= width {
            used += span_width;
            out.push(span.clone());
            continue;
        }
        let mut cut = String::new();
        for ch in span.content.chars() {
            let ch_width = ch.width().unwrap_or(0);
            if used + ch_width > width {
                break;
            }
            used += ch_width;
            cut.push(ch);
        }
        out.push(Span::styled(cut, span.style));
        break;
    }
    out
}

/// Append a trailing span of `selection_bg`-styled spaces so the row's
/// highlight extends to `width`. No-op when the row is already wider
/// than the pane (ratatui clips overflow).
fn pad_selected_row(line: &mut Line<'static>, width: usize, theme: &Theme) {
    let current: usize = line.spans.iter().map(|s| s.content.width()).sum();
    if current >= width {
        return;
    }
    let pad = width - current;
    line.spans.push(Span::styled(
        " ".repeat(pad),
        Style::default().bg(rgb_to_color(theme.selection_bg)),
    ));
}

/// Build the sidebar pane's footer: how many files have staged changes,
/// then the aggregate `+N -N` line counts in the added and deleted
/// colours, as on each row. The count says in words what the green and
/// amber names say in colour.
///
/// Returns `None` when no lines changed.
pub(super) fn sidebar_footer(sidebar: &Sidebar, theme: &Theme) -> Option<Line<'static>> {
    let totals = sidebar.totals();
    if totals.added == 0 && totals.deleted == 0 {
        return None;
    }
    let mut spans = vec![Span::raw(" ")];
    let staged = sidebar.staged_count();
    if staged > 0 {
        spans.push(Span::styled(
            format!("{staged} staged"),
            Style::default().fg(rgb_to_color(theme.status_added)),
        ));
        spans.push(Span::raw(" \u{b7} "));
    }
    if totals.added > 0 {
        spans.push(Span::styled(
            format!("+{}", totals.added),
            Style::default().fg(rgb_to_color(theme.status_added)),
        ));
    }
    if totals.added > 0 && totals.deleted > 0 {
        spans.push(Span::raw(" "));
    }
    if totals.deleted > 0 {
        spans.push(Span::styled(
            format!("-{}", totals.deleted),
            Style::default().fg(rgb_to_color(theme.status_deleted)),
        ));
    }
    spans.push(Span::raw(" "));
    Some(Line::from(spans))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::browse::files::model::ResolvedFile;
    use crate::cli::browse::files::test_support::*;
    use crate::cli::browse::files::{Focus, handle_key, handle_mouse};
    use crossterm::event::{MouseButton, MouseEventKind};

    fn drawn_rows(sidebar: &Sidebar, width: u16) -> Vec<String> {
        let backend = ratatui::backend::TestBackend::new(width, 7);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                draw_sidebar(
                    frame,
                    frame.area(),
                    sidebar,
                    pane::Chrome::new(Line::from("Files"), true),
                    &Theme::default(),
                )
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        (2..6)
            .map(|y| {
                (1..width - 1)
                    .map(|x| buffer[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect()
    }

    fn noted(paths: &[&str], note: crate::sidebar::RowNote) -> Sidebar {
        let diffs: Vec<_> = paths.iter().map(|path| file_diff(path)).collect();
        let files: Vec<_> = diffs
            .iter()
            .map(|file| SidebarFile {
                file,
                added: 1,
                deleted: 1,
                stage: None,
            })
            .collect();
        let mut sidebar =
            Sidebar::build_with_icons(&files, &Theme::default(), crate::sidebar::IconMode::Off);
        sidebar.update_notes(&vec![Some(note); paths.len()]);
        sidebar
    }

    #[test]
    fn notes_sit_at_the_right_edge_and_line_counts_give_way_first() {
        let note = crate::sidebar::RowNote {
            tag: "core",
            level: 1,
            low: false,
        };
        let sidebar = noted(&["a.rs", "much_longer_name.rs"], note);

        let rows = drawn_rows(&sidebar, 32);

        assert_eq!(rows[0], format!("M a.rs +1 -1{}core ●", " ".repeat(12)));
        assert_eq!(
            rows[1],
            format!("M much_longer_name.rs{}core ●", " ".repeat(3))
        );
    }

    #[test]
    fn a_long_name_is_cut_so_the_note_stays_visible() {
        let note = crate::sidebar::RowNote {
            tag: "core",
            level: 1,
            low: false,
        };
        let sidebar = noted(&["a_really_long_file_name.rs"], note);

        let rows = drawn_rows(&sidebar, 20);

        assert_eq!(rows[0], "M a_really_ core ●");
    }

    fn sidebar_with_counts(counts: &[(usize, usize)]) -> Sidebar {
        let diffs: Vec<_> = (0..counts.len())
            .map(|i| file_diff(&format!("f{i}.rs")))
            .collect();
        let files: Vec<_> = diffs
            .iter()
            .zip(counts)
            .map(|(file, &(added, deleted))| SidebarFile {
                file,
                added,
                deleted,
                stage: None,
            })
            .collect();
        Sidebar::build_with_icons(&files, &Theme::default(), crate::sidebar::IconMode::Off)
    }

    #[test]
    fn footer_shows_total_line_counts_in_added_and_deleted_colours() {
        let theme = Theme::default();
        let sidebar = sidebar_with_counts(&[(3, 1), (2, 4)]);

        let line = sidebar_footer(&sidebar, &theme).unwrap();

        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, " +5 -5 ");
        let added = line.spans.iter().find(|s| s.content == "+5").unwrap();
        assert_eq!(added.style.fg, Some(rgb_to_color(theme.status_added)));
        let deleted = line.spans.iter().find(|s| s.content == "-5").unwrap();
        assert_eq!(deleted.style.fg, Some(rgb_to_color(theme.status_deleted)));
    }

    #[test]
    fn footer_counts_files_with_staged_changes() {
        use crate::sidebar::{ChangeKind, StageStatus};
        let diffs = [file_diff("a.rs"), file_diff("b.rs"), file_diff("c.rs")];
        let stages = [
            (Some(ChangeKind::Modified), None),
            (Some(ChangeKind::Modified), Some(ChangeKind::Modified)),
            (None, Some(ChangeKind::Modified)),
        ];
        let files: Vec<_> = diffs
            .iter()
            .zip(stages)
            .map(|(file, (staged, unstaged))| SidebarFile {
                file,
                added: 1,
                deleted: 0,
                stage: Some(StageStatus { staged, unstaged }),
            })
            .collect();
        let sidebar =
            Sidebar::build_with_icons(&files, &Theme::default(), crate::sidebar::IconMode::Off);

        let line = sidebar_footer(&sidebar, &Theme::default()).unwrap();

        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, " 2 staged \u{b7} +3 ");
    }

    #[test]
    fn footer_leaves_out_a_side_with_no_lines() {
        let sidebar = sidebar_with_counts(&[(2, 0)]);

        let line = sidebar_footer(&sidebar, &Theme::default()).unwrap();

        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, " +2 ");
    }

    #[test]
    fn handle_key_j_in_sidebar_focus_moves_sidebar_and_snaps_diff() {
        let a = file_diff("a.txt");
        let b = file_diff("b.txt");
        let resolved = vec![
            ResolvedFile {
                file: a,
                before: "a1\n".to_string(),
                after: "a2\n".to_string(),
            },
            ResolvedFile {
                file: b,
                before: "b1\n".to_string(),
                after: "b2\n".to_string(),
            },
        ];
        let mut state = make_state(&resolved);
        assert_eq!(state.focus, Focus::Sidebar);
        // Initial selection is file 0, diff_scroll 0.
        assert_eq!(state.sidebar.selected_file_index(), Some(0));
        state.unstaged.cursor.scroll = 5; // scrolled somewhere inside file 0

        handle_key(&mut state, KeyCode::Char('j'), 4);
        // Sidebar should now be on file 1.
        assert_eq!(state.sidebar.selected_file_index(), Some(1));
        // The diff snapped to the top of the newly selected file's window.
        assert_eq!(state.unstaged.cursor.scroll, 0);
    }

    #[test]
    fn enter_in_sidebar_folds_and_unfolds_the_selected_directory() {
        let resolved: Vec<_> = ["src/a.txt", "src/b.txt", "z.txt"]
            .into_iter()
            .map(|path| ResolvedFile {
                file: file_diff(path),
                before: "1\n".to_string(),
                after: "2\n".to_string(),
            })
            .collect();
        let mut state = make_state(&resolved);
        state.sidebar.set_selected(0, 4);

        handle_key(&mut state, KeyCode::Enter, 4);
        assert_eq!(state.sidebar.row_count(), 2);
        assert_eq!(state.sidebar.selection_display_range(), Some(0..2));

        handle_key(&mut state, KeyCode::Enter, 4);
        assert_eq!(state.sidebar.row_count(), 4);
    }

    #[test]
    fn scroll_down_on_sidebar_moves_selection() {
        let a = file_diff("a.txt");
        let b = file_diff("b.txt");
        let resolved = vec![
            ResolvedFile {
                file: a,
                before: "a1\n".to_string(),
                after: "a2\n".to_string(),
            },
            ResolvedFile {
                file: b,
                before: "b1\n".to_string(),
                after: "b2\n".to_string(),
            },
        ];
        let mut state = make_state_with_rects(&resolved);
        let initial = state.sidebar.selected();

        let mouse = make_mouse(MouseEventKind::ScrollDown, 5, 5);
        handle_mouse(&mut state, mouse, 18);
        assert!(state.sidebar.selected() > initial);
    }

    #[test]
    fn sidebar_burst_scroll_moves_one_row_per_tick() {
        // A single physical wheel tick fans out into a burst of events; the
        // shared WheelScroll collapses one quota's worth of events into a
        // single selection move, so the sidebar steps slowly rather than
        // jumping several rows per tick.
        let files: Vec<_> = (0..6).map(|i| file_diff(&format!("f{i}.txt"))).collect();
        let resolved: Vec<ResolvedFile> = files
            .into_iter()
            .map(|f| ResolvedFile {
                file: f,
                before: "a\n".to_string(),
                after: "b\n".to_string(),
            })
            .collect();
        let mut state = make_state_with_rects(&resolved);
        let initial = state.sidebar.selected();

        for _ in 0..3 {
            handle_mouse(&mut state, make_mouse(MouseEventKind::ScrollDown, 5, 5), 18);
        }
        assert_eq!(state.sidebar.selected(), initial + 1);
    }

    #[test]
    fn click_on_sidebar_selects_row() {
        let a = file_diff("a.txt");
        let b = file_diff("b.txt");
        let resolved = vec![
            ResolvedFile {
                file: a,
                before: "a1\n".to_string(),
                after: "a2\n".to_string(),
            },
            ResolvedFile {
                file: b,
                before: "b1\n".to_string(),
                after: "b2\n".to_string(),
            },
        ];
        let mut state = make_state_with_rects(&resolved);
        let row_count = state.sidebar.row_count();
        assert!(row_count >= 2, "need at least 2 rows for this test");

        // The sidebar's body starts below its title row and rule.
        // Click on the last row.
        let target_row = row_count - 1;
        let mouse_y = 2 + target_row as u16;
        let mouse = make_mouse(MouseEventKind::Down(MouseButton::Left), 5, mouse_y);
        handle_mouse(&mut state, mouse, 18);
        assert_eq!(state.sidebar.selected(), target_row,);
        assert_eq!(state.focus, Focus::Sidebar);
    }

    #[test]
    fn click_on_a_directory_arrow_folds_it() {
        let resolved: Vec<_> = ["src/a.txt", "src/b.txt"]
            .into_iter()
            .map(|path| ResolvedFile {
                file: file_diff(path),
                before: "1\n".to_string(),
                after: "2\n".to_string(),
            })
            .collect();
        let mut state = make_state_with_rects(&resolved);
        assert_eq!(state.sidebar.row_count(), 3);

        let arrow = make_mouse(MouseEventKind::Down(MouseButton::Left), 1, 1);
        handle_mouse(&mut state, arrow, 18);
        assert_eq!(state.sidebar.row_count(), 1);
        assert_eq!(state.focus, Focus::Sidebar);
    }
}

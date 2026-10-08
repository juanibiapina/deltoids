//! Sidebar pane vertical slice: builds the [`Sidebar`] model from a
//! [`Model`], handles its movement keys, and renders it (rows, selection
//! bar, footer). Selection-driven scrolling of the diff pane is
//! coordination owned by the shell, not this slice.

use crossterm::event::KeyCode;
use ratatui::layout::{Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use deltoids::Theme;
use deltoids::render_tui::{
    pane_block_with_title_line, pane_border_color, pane_inner_height, render_pane_scrollbar,
    rgb_to_color,
};

use crate::sidebar::{Sidebar, SidebarFile, display_path};

use super::model::{Model, body_deltas};

/// The sidebar's top border title in lazygit's style: `─[1]─Files─`. The
/// `[1]` badge and the label use the bold accent; the `─` rules use
/// `rule_color`, the pane's own border colour, so the title stays
/// continuous with the border (accent when focused, plain otherwise).
pub(crate) fn sidebar_title(rule_color: Color, theme: &Theme) -> Line<'static> {
    let rule = Style::default().fg(rule_color);
    let accent = Style::default()
        .fg(rgb_to_color(theme.border_active))
        .add_modifier(Modifier::BOLD);
    Line::from(vec![
        Span::styled("─", rule),
        Span::styled("[1]", accent),
        Span::styled("─", rule),
        Span::styled("Files", accent),
        Span::styled("─", rule),
    ])
}

/// Build the sidebar from a model plus per-file delta counts.
pub(super) fn build_sidebar(model: &Model, theme: &Theme) -> Sidebar {
    Sidebar::build(&sidebar_files(model), theme)
}

pub(super) fn rebuild_sidebar(sidebar: &Sidebar, model: &Model, hidden: &[bool]) -> Sidebar {
    sidebar.rebuilt(&sidebar_files(model), hidden)
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
    footer: Option<String>,
    focused: bool,
    theme: &Theme,
) {
    let inner = area.inner(Margin {
        vertical: 1,
        horizontal: 1,
    });
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
            let fill = if start + offset == sidebar.selected() {
                selected_bg
            } else {
                Style::default()
            };
            with_note_at_right(row, note.as_ref(), inner_width, fill)
        })
        .collect();

    // Extend the selection background across the full inner pane width
    // so the highlighted row reads as a continuous bar (matching
    // lazygit's `List` widget). Pad against the inner
    // width so the trailing block stops just before the right border.
    if let Some(rel) = sidebar.selected().checked_sub(scroll)
        && rel < visible.len()
    {
        pad_selected_row(&mut visible[rel], inner_width, theme);
    }

    let color = pane_border_color(focused, theme);
    let block = pane_block_with_title_line(sidebar_title(color, theme), color, footer);
    frame.render_widget(Paragraph::new(visible).block(block), area);

    render_pane_scrollbar(
        frame,
        area,
        total,
        sidebar.selected(),
        pane_inner_height(area),
        focused,
        theme,
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

/// Build the sidebar pane's bottom-right footer: file/dir position
/// among all files plus the aggregate `+N -N` line counts.
///
/// Returns `None` when there are no files to display.
pub(super) fn sidebar_footer(sidebar: &Sidebar, display_order: &[usize]) -> Option<String> {
    let total = display_order.len();
    if total == 0 {
        return None;
    }
    let selected_input = sidebar.nearest_file_index()?;
    let pos = display_order
        .iter()
        .position(|&i| i == selected_input)
        .map(|p| p + 1)
        .unwrap_or(0);
    let label = if sidebar.selected_is_dir() {
        "dir"
    } else {
        "file"
    };
    let totals = sidebar.totals();
    let mut s = format!(" {label} {pos} of {total}");
    if totals.added > 0 || totals.deleted > 0 {
        s.push_str("  ");
        if totals.added > 0 {
            s.push_str(&format!("+{}", totals.added));
            if totals.deleted > 0 {
                s.push(' ');
            }
        }
        if totals.deleted > 0 {
            s.push_str(&format!("-{}", totals.deleted));
        }
    }
    s.push(' ');
    Some(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::browse::files::model::ResolvedFile;
    use crate::cli::browse::files::test_support::*;
    use crate::cli::browse::files::{Focus, handle_key, handle_mouse};
    use crossterm::event::{MouseButton, MouseEventKind};

    fn drawn_rows(sidebar: &Sidebar, width: u16) -> Vec<String> {
        let backend = ratatui::backend::TestBackend::new(width, 6);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| draw_sidebar(frame, frame.area(), sidebar, None, true, &Theme::default()))
            .unwrap();
        let buffer = terminal.backend().buffer();
        (1..5)
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
            breaking: false,
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
            breaking: false,
        };
        let sidebar = noted(&["a_really_long_file_name.rs"], note);

        let rows = drawn_rows(&sidebar, 20);

        assert_eq!(rows[0], "M a_really_ core ●");
    }

    #[test]
    fn sidebar_title_reads_files_with_accent_label_and_border_rules() {
        let theme = Theme::default();
        let rule_color = rgb_to_color(theme.border);
        let line = sidebar_title(rule_color, &theme);
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "─[1]─Files─");
        let files = line.spans.iter().find(|s| s.content == "Files").unwrap();
        assert_eq!(files.style.fg, Some(rgb_to_color(theme.border_active)));
        assert!(files.style.add_modifier.contains(Modifier::BOLD));
        let rule = line.spans.iter().find(|s| s.content == "─").unwrap();
        assert_eq!(rule.style.fg, Some(rule_color));
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

        // Sidebar rect starts at y=0, so row 1 = border,
        // row 2 = second content row (index 1). Click on the last row.
        let target_row = row_count - 1;
        let mouse_y = 1 + target_row as u16; // +1 for top border
        let mouse = make_mouse(MouseEventKind::Down(MouseButton::Left), 5, mouse_y);
        handle_mouse(&mut state, mouse, 18);
        assert_eq!(state.sidebar.selected(), target_row,);
        assert_eq!(state.focus, Focus::Sidebar);
    }
}

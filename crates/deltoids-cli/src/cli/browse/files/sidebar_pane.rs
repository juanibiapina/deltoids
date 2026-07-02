//! Sidebar pane vertical slice: builds the [`Sidebar`] model from a
//! [`Model`], handles its movement keys, and renders it (rows, selection
//! bar, footer). Selection-driven scrolling of the diff pane is
//! coordination owned by the shell, not this slice.

use crossterm::event::KeyCode;
use ratatui::layout::{Margin, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

use deltoids::Theme;
use deltoids::render_tui::{
    pane_block_with_tabs, pane_border_color, pane_inner_height, render_pane_scrollbar, rgb_to_color,
};

use crate::sidebar::{Sidebar, SidebarFile};

use super::model::{Model, count_deltas};

/// Build the sidebar from a model plus per-file delta counts.
pub(super) fn build_sidebar(model: &Model, theme: &Theme) -> Sidebar {
    let sidebar_files: Vec<SidebarFile<'_>> = model
        .files
        .iter()
        .zip(model.diffs.iter())
        .map(|(f, d)| {
            let (added, deleted) = count_deltas(d);
            SidebarFile {
                file: &f.file,
                added,
                deleted,
            }
        })
        .collect();
    Sidebar::build(&sidebar_files, theme)
}

/// Handle a movement key while the sidebar is focused. Returns `true`
/// when the selection moved, so the shell can snap the diff pane to the
/// newly selected file (the cross-pane coordination it owns).
pub(super) fn handle_key(sidebar: &mut Sidebar, key: KeyCode, viewport: usize) -> bool {
    match key {
        KeyCode::Char('j') | KeyCode::Down => sidebar.move_down(viewport),
        KeyCode::Char('k') | KeyCode::Up => sidebar.move_up(viewport),
        KeyCode::PageDown => sidebar.page_down(viewport),
        KeyCode::PageUp => sidebar.page_up(viewport),
        KeyCode::Char('g') | KeyCode::Home => sidebar.top(viewport),
        KeyCode::Char('G') | KeyCode::End => sidebar.bottom(viewport),
        _ => return false,
    }
    true
}

pub(super) fn draw_sidebar(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    sidebar: &Sidebar,
    display_order: &[usize],
    focused: bool,
    title: Line<'static>,
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
    let mut visible: Vec<Line<'static>> = sidebar.rows()[start..end].to_vec();

    // Extend the selection background across the full inner pane width
    // so the highlighted row reads as a continuous bar (matching
    // lazygit and the traces TUI's `List` widget). Pad against the inner
    // width so the trailing block stops just before the right border.
    if let Some(rel) = sidebar.selected().checked_sub(scroll)
        && rel < visible.len()
    {
        pad_selected_row(&mut visible[rel], inner_width, theme);
    }

    let color = pane_border_color(focused, theme);
    let footer = sidebar_footer(sidebar, display_order);
    let block = pane_block_with_tabs(title, color, footer);
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
        state.diff.diff_scroll = 5; // scrolled somewhere inside file 0

        handle_key(&mut state, KeyCode::Char('j'), 2, 4);
        // Sidebar should now be on file 1.
        assert_eq!(state.sidebar.selected_file_index(), Some(1));
        // The diff snapped to the top of the newly selected file's window.
        assert_eq!(state.diff.diff_scroll, 0);
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
        handle_mouse(&mut state, mouse, 18, 18);
        assert!(state.sidebar.selected() > initial);
    }

    #[test]
    fn sidebar_burst_scroll_moves_one_row_per_tick() {
        // A single physical wheel tick fans out into a burst of events; the
        // shared WheelScroll collapses one quota's worth of events into a
        // single selection move, so the sidebar steps slowly like the traces
        // lists rather than jumping several rows per tick.
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
            handle_mouse(
                &mut state,
                make_mouse(MouseEventKind::ScrollDown, 5, 5),
                18,
                18,
            );
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
        handle_mouse(&mut state, mouse, 18, 18);
        assert_eq!(state.sidebar.selected(), target_row,);
        assert_eq!(state.focus, Focus::Sidebar);
    }
}

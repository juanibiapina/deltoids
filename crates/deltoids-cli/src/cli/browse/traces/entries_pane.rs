//! Entries pane slice: selection movement within the active trace's
//! entries, the entry-row labels, and the pane render.

use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{List, ListItem},
};
use std::path::Path;

use deltoids::Theme;
use deltoids::render_tui::{
    pane_block_with_tabs, pane_border_color, pane_inner_height, position_footer,
    render_pane_scrollbar, rgb_to_color,
};

use crate::HistoryEntry;

use super::model::LoadedTrace;
use super::{AppState, Focus};

pub(super) fn move_entry_down(state: &mut AppState, traces: &[LoadedTrace]) {
    let entry_count = traces
        .get(state.trace_index)
        .map(|trace| trace.entries.len())
        .unwrap_or(0);
    let current = state.entry_index();
    if current + 1 < entry_count {
        state.set_entry_index(current + 1);
        state.diff_scroll = 0;
    }
}

pub(super) fn move_entry_up(state: &mut AppState) {
    let current = state.entry_index();
    if current > 0 {
        state.set_entry_index(current - 1);
        state.diff_scroll = 0;
    }
}

pub(super) fn render_entries_pane(
    frame: &mut ratatui::Frame<'_>,
    area: ratatui::layout::Rect,
    active_trace: &LoadedTrace,
    state: &mut AppState,
    title: Line<'static>,
    theme: &Theme,
) {
    let entry_items = active_trace
        .entries
        .iter()
        .map(|entry| ListItem::new(entry_label_line(entry)))
        .collect::<Vec<_>>();
    let entries_count = active_trace.entries.len();
    let entries_position = if entries_count == 0 {
        0
    } else {
        state.entry_index() + 1
    };
    let entries_list = List::new(entry_items)
        .block(pane_block_with_tabs(
            title,
            pane_border_color(state.focus == Focus::Entries, theme),
            Some(position_footer(entries_position, entries_count)),
        ))
        .highlight_style(
            Style::default()
                .bg(rgb_to_color(theme.selection_bg))
                .add_modifier(Modifier::BOLD),
        )
        .scroll_padding(2);
    frame.render_stateful_widget(entries_list, area, &mut state.entries_list_state);
    render_pane_scrollbar(
        frame,
        area,
        entries_count,
        state.entry_index(),
        pane_inner_height(area),
        theme,
    );
}

fn entry_icon(ok: bool) -> (&'static str, Color) {
    if ok {
        ("\u{2713}", Color::Green)
    } else {
        ("\u{2717}", Color::Red)
    }
}

fn file_basename(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string())
}

fn entry_label_line(entry: &HistoryEntry) -> Line<'static> {
    let (icon, icon_color) = entry_icon(entry.ok);
    let basename = file_basename(&entry.path);
    Line::from(vec![
        Span::styled(icon.to_string(), Style::default().fg(icon_color)),
        Span::raw(format!(" {basename} ")),
        Span::styled(entry.reason.clone(), Style::default().fg(Color::DarkGray)),
    ])
}

pub(super) fn entry_label_plain(entry: &HistoryEntry) -> String {
    let (icon, _) = entry_icon(entry.ok);
    format!("{icon} {} {}", file_basename(&entry.path), entry.reason)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::browse::traces::test_support::*;
    use crate::cli::browse::traces::{handle_key, handle_mouse};
    use crossterm::event::{KeyCode, MouseButton, MouseEventKind};

    #[test]
    fn j_moves_entries_when_focused_on_entries() {
        let traces = vec![LoadedTrace {
            trace: trace_summary("01JTESTTRACE00000000000000", 2, "a"),
            entries: vec![edit_entry(), write_entry()],
        }];
        let mut state = AppState::new(traces.len());
        state.focus = Focus::Entries;

        handle_key(&mut state, &traces, KeyCode::Char('j'), 0, 0);
        assert_eq!(state.entry_index(), 1);
        assert_eq!(state.trace_index, 0);
    }

    #[test]
    fn scroll_down_on_entries_pane_moves_entry_selection() {
        let traces = vec![LoadedTrace {
            trace: trace_summary("01JTESTTRACE00000000000000", 3, "a"),
            entries: vec![edit_entry(), edit_entry(), edit_entry()],
        }];
        let mut state = state_with_rects(&traces);
        state.focus = Focus::Diff;
        assert_eq!(state.entry_index(), 0);

        let mouse = make_mouse(MouseEventKind::ScrollDown, 5, 3);
        handle_mouse(&mut state, &traces, mouse, 20, 10);
        assert_eq!(state.entry_index(), 1);
        assert_eq!(state.focus, Focus::Diff);
    }

    #[test]
    fn scroll_up_on_entries_pane_moves_entry_selection() {
        let traces = vec![LoadedTrace {
            trace: trace_summary("01JTESTTRACE00000000000000", 3, "a"),
            entries: vec![edit_entry(), edit_entry(), edit_entry()],
        }];
        let mut state = state_with_rects(&traces);
        state.set_entry_index(2);

        let mouse = make_mouse(MouseEventKind::ScrollUp, 5, 3);
        handle_mouse(&mut state, &traces, mouse, 20, 10);
        assert_eq!(state.entry_index(), 1);
    }

    #[test]
    fn entries_burst_scroll_moves_one_item_per_tick() {
        let traces = vec![LoadedTrace {
            trace: trace_summary("01JTESTTRACE00000000000000", 4, "a"),
            entries: vec![edit_entry(), edit_entry(), edit_entry(), edit_entry()],
        }];
        let mut state = state_with_rects(&traces);
        assert_eq!(state.entry_index(), 0);

        for _ in 0..3 {
            handle_mouse(
                &mut state,
                &traces,
                make_mouse(MouseEventKind::ScrollDown, 5, 3),
                20,
                10,
            );
        }
        assert_eq!(state.entry_index(), 1);
    }

    #[test]
    fn click_on_entry_selects_it() {
        let traces = vec![LoadedTrace {
            trace: trace_summary("01JTESTTRACE00000000000000", 3, "a"),
            entries: vec![edit_entry(), edit_entry(), edit_entry()],
        }];
        let mut state = state_with_rects(&traces);
        assert_eq!(state.entry_index(), 0);

        // Click on row 2 inside entries pane (rect starts at y=0, +1 border = row 1 is first item).
        // Row 3 = content_y 2 = item index 2.
        let mouse = make_mouse(MouseEventKind::Down(MouseButton::Left), 5, 3);
        handle_mouse(&mut state, &traces, mouse, 20, 10);
        assert_eq!(state.entry_index(), 2);
        assert_eq!(state.focus, Focus::Entries);
    }
}

//! Traces pane slice: selection movement across traces, the trace-row
//! label, and the pane render.

use ratatui::{
    style::{Modifier, Style},
    widgets::{List, ListItem},
};

use deltoids::Theme;
use deltoids::render_tui::{
    pane_block_with_footer, pane_border_color, pane_inner_height, position_footer,
    render_pane_scrollbar, rgb_to_color,
};

use crate::TraceSummary;

use super::model::LoadedTrace;
use super::{AppState, Focus};

pub(super) fn move_trace_down(state: &mut AppState, traces: &[LoadedTrace]) {
    if state.trace_index + 1 < traces.len() {
        state.trace_index += 1;
        state.traces_list_state.select(Some(state.trace_index));
        state.diff_scroll = 0;
    }
}

pub(super) fn move_trace_up(state: &mut AppState) {
    if state.trace_index > 0 {
        state.trace_index -= 1;
        state.traces_list_state.select(Some(state.trace_index));
        state.diff_scroll = 0;
    }
}

pub(super) fn render_traces_pane(
    frame: &mut ratatui::Frame<'_>,
    area: ratatui::layout::Rect,
    traces: &[LoadedTrace],
    state: &mut AppState,
    theme: &Theme,
) {
    let trace_items = traces
        .iter()
        .map(|loaded| ListItem::new(trace_label(&loaded.trace)))
        .collect::<Vec<_>>();
    let traces_count = traces.len();
    let traces_position = if traces_count == 0 {
        0
    } else {
        state.trace_index + 1
    };
    let traces_list = List::new(trace_items)
        .block(pane_block_with_footer(
            "─[2]─Traces─",
            pane_border_color(state.focus == Focus::Traces, theme),
            Some(position_footer(traces_position, traces_count)),
        ))
        .highlight_style(
            Style::default()
                .bg(rgb_to_color(theme.selection_bg))
                .add_modifier(Modifier::BOLD),
        )
        .scroll_padding(2);
    frame.render_stateful_widget(traces_list, area, &mut state.traces_list_state);
    render_pane_scrollbar(
        frame,
        area,
        traces_count,
        state.trace_index,
        pane_inner_height(area),
        theme,
    );
}

pub(super) fn trace_label(summary: &TraceSummary) -> String {
    let short_id = short_trace_id(&summary.trace_id);
    format!(
        "{}  {} entries  {}  {}",
        short_id, summary.entry_count, summary.last_timestamp, summary.last_reason
    )
}

fn short_trace_id(trace_id: &str) -> String {
    if trace_id.len() <= 10 {
        trace_id.to_string()
    } else {
        trace_id[..10].to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::browse::traces::test_support::*;
    use crate::cli::browse::traces::{handle_key, handle_mouse};
    use crossterm::event::{KeyCode, MouseButton, MouseEventKind};

    #[test]
    fn j_moves_traces_when_focused_on_traces() {
        let traces = vec![
            LoadedTrace {
                trace: trace_summary("01JTESTTRACE00000000000000", 1, "a"),
                entries: vec![edit_entry()],
            },
            LoadedTrace {
                trace: trace_summary("01JTESTTRACE00000000000001", 1, "b"),
                entries: vec![edit_entry()],
            },
        ];
        let mut state = AppState::new(traces.len());
        state.focus = Focus::Traces;

        handle_key(&mut state, &traces, KeyCode::Char('j'), 0, 0);
        assert_eq!(state.trace_index, 1);
        assert_eq!(state.entry_index(), 0);
    }

    #[test]
    fn scroll_down_on_traces_pane_moves_trace_selection() {
        let traces = vec![
            LoadedTrace {
                trace: trace_summary("01JTESTTRACE00000000000000", 1, "a"),
                entries: vec![edit_entry()],
            },
            LoadedTrace {
                trace: trace_summary("01JTESTTRACE00000000000001", 1, "b"),
                entries: vec![edit_entry()],
            },
        ];
        let mut state = state_with_rects(&traces);
        assert_eq!(state.trace_index, 0);

        let mouse = make_mouse(MouseEventKind::ScrollDown, 5, 15);
        handle_mouse(&mut state, &traces, mouse, 20, 10);
        assert_eq!(state.trace_index, 1);
    }

    #[test]
    fn traces_burst_scroll_moves_one_item_per_tick() {
        let traces = vec![
            LoadedTrace {
                trace: trace_summary("01JTESTTRACE00000000000000", 1, "a"),
                entries: vec![edit_entry()],
            },
            LoadedTrace {
                trace: trace_summary("01JTESTTRACE00000000000001", 1, "b"),
                entries: vec![edit_entry()],
            },
            LoadedTrace {
                trace: trace_summary("01JTESTTRACE00000000000002", 1, "c"),
                entries: vec![edit_entry()],
            },
        ];
        let mut state = state_with_rects(&traces);
        assert_eq!(state.trace_index, 0);

        for _ in 0..3 {
            handle_mouse(
                &mut state,
                &traces,
                make_mouse(MouseEventKind::ScrollDown, 5, 15),
                20,
                10,
            );
        }
        assert_eq!(state.trace_index, 1);
    }

    #[test]
    fn click_on_trace_selects_it() {
        let traces = vec![
            LoadedTrace {
                trace: trace_summary("01JTESTTRACE00000000000000", 1, "a"),
                entries: vec![edit_entry()],
            },
            LoadedTrace {
                trace: trace_summary("01JTESTTRACE00000000000001", 1, "b"),
                entries: vec![edit_entry()],
            },
        ];
        let mut state = state_with_rects(&traces);
        assert_eq!(state.trace_index, 0);

        // Click on row 12 inside traces pane (rect starts at y=10, +1 border = row 11 is first item).
        // Row 12 = content_y 1 = item index 1.
        let mouse = make_mouse(MouseEventKind::Down(MouseButton::Left), 5, 12);
        handle_mouse(&mut state, &traces, mouse, 20, 10);
        assert_eq!(state.trace_index, 1);
        assert_eq!(state.focus, Focus::Traces);
    }
}

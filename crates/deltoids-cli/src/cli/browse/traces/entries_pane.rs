//! Entries pane slice: selection movement within the active trace's
//! entries, the entry-row labels, and the pane render.

use deltoids::Theme;
use deltoids::render_tui::{
    pane_block_with_tabs, pane_border_color, pane_inner_height, position_footer,
    render_pane_scrollbar, rgb_to_color,
};
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{List, ListItem},
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
        state.reset_diff_view();
    }
}

pub(super) fn move_entry_up(state: &mut AppState) {
    let current = state.entry_index();
    if current > 0 {
        state.set_entry_index(current - 1);
        state.reset_diff_view();
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
        .map(|entry| ListItem::new(entry_label_line(entry, theme)))
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
        state.focus == Focus::Entries,
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

fn entry_path_parts(entry: &HistoryEntry) -> (String, String) {
    let path = super::detail::display_path(&entry.path, &entry.cwd);
    match path.rsplit_once('/') {
        Some((parent, filename)) if !filename.is_empty() => (
            filename.to_string(),
            if parent.is_empty() { "/" } else { parent }.to_string(),
        ),
        _ => (path, String::new()),
    }
}

fn entry_label_line(entry: &HistoryEntry, theme: &Theme) -> Line<'static> {
    let (icon, icon_color) = entry_icon(entry.ok);
    let (filename, parent) = entry_path_parts(entry);
    let mut spans = vec![
        Span::styled(icon.to_string(), Style::default().fg(icon_color)),
        Span::raw(format!(" {filename}")),
    ];
    if !parent.is_empty() {
        spans.push(Span::styled(
            format!("  {parent}"),
            Style::default().fg(rgb_to_color(theme.muted)),
        ));
    }
    Line::from(spans)
}

pub(super) fn entry_label_plain(entry: &HistoryEntry) -> String {
    let (icon, _) = entry_icon(entry.ok);
    let (filename, parent) = entry_path_parts(entry);
    if parent.is_empty() {
        format!("{icon} {filename}")
    } else {
        format!("{icon} {filename}  {parent}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::browse::traces::test_support::*;
    use crate::cli::browse::traces::{handle_key, handle_mouse};
    use crossterm::event::{KeyCode, MouseButton, MouseEventKind};

    #[test]
    fn entries_render_filenames_before_muted_directories() {
        use ratatui::{Terminal, backend::TestBackend};

        let theme = test_theme();
        let mut nested = edit_entry();
        nested.path = "/tmp/project/src/main.rs".to_string();
        let mut other = nested.clone();
        other.path = "/tmp/project/tests/main.rs".to_string();
        other.ok = false;
        let mut outside = nested.clone();
        outside.path = "/outside/main.rs".to_string();
        let mut relative = nested.clone();
        relative.path = "lib/main.rs".to_string();
        let mut fallback = nested.clone();
        fallback.path = "/".to_string();
        let trace = LoadedTrace {
            trace: trace_summary("trace", 6, "description"),
            entries: vec![nested, other, write_entry(), outside, relative, fallback],
        };
        let labels = [
            "✓ main.rs  src",
            "✗ main.rs  tests",
            "✓ config.json",
            "✓ main.rs  /outside",
            "✓ main.rs  lib",
            "✓ /",
        ];
        for (entry, label) in trace.entries.iter().zip(labels) {
            assert_eq!(entry_label_plain(entry), label);
        }

        for width in [40, 11] {
            let mut terminal = Terminal::new(TestBackend::new(width, 10)).unwrap();
            let mut state = AppState::new(1);
            terminal
                .draw(|frame| {
                    render_entries_pane(
                        frame,
                        frame.area(),
                        &trace,
                        &mut state,
                        Line::from("Entries"),
                        &theme,
                    );
                })
                .unwrap();
            let buffer = terminal.backend().buffer();
            for (index, label) in labels.iter().enumerate() {
                let visible: String = (1..width - 1)
                    .map(|x| buffer[(x, index as u16 + 1)].symbol())
                    .collect();
                let expected: String = label.chars().take((width - 2) as usize).collect();
                assert_eq!(visible.trim_end(), expected.trim_end());
            }
            assert_eq!(buffer[(1, 1)].fg, Color::Green);
            assert_eq!(buffer[(1, 2)].fg, Color::Red);
            assert_eq!(buffer[(3, 1)].bg, rgb_to_color(theme.selection_bg));
            if width == 40 {
                for y in [1, 2, 4, 5] {
                    assert_eq!(buffer[(12, y)].fg, rgb_to_color(theme.muted));
                }
            }
        }
    }

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

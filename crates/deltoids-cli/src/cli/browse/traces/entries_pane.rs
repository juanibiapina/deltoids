//! Entries pane slice: the rows of the active trace (one per entry, plus
//! one per file under a multi-file entry), selection movement over those
//! rows, their labels, and the pane render.

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
use super::{AppState, Focus, Selection};

/// One row of the entries pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum EntryRow {
    /// An entry. Labeled by its file when it changed exactly one file,
    /// otherwise by its reason.
    Entry(usize),
    /// One file of a multi-file entry, listed under it.
    File(usize, usize),
}

impl EntryRow {
    pub(super) fn selection(self) -> Selection {
        match self {
            EntryRow::Entry(entry) => Selection::entry(entry),
            EntryRow::File(entry, file) => Selection {
                entry,
                file: Some(file),
            },
        }
    }
}

/// Every row for `entries`, in display order. An entry named by its file is
/// one row; an entry named by its reason lists its files under it.
pub(super) fn entry_rows(entries: &[HistoryEntry]) -> Vec<EntryRow> {
    let mut rows = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        rows.push(EntryRow::Entry(index));
        if let EntryName::Reason(_) = entry_name(entry) {
            rows.extend((0..entry.files.len()).map(|file| EntryRow::File(index, file)));
        }
    }
    rows
}

/// The row showing `selection`, falling back to its entry's row.
fn selected_row(rows: &[EntryRow], selection: Selection) -> Option<usize> {
    rows.iter()
        .position(|row| row.selection() == selection)
        .or_else(|| {
            rows.iter()
                .position(|row| *row == EntryRow::Entry(selection.entry))
        })
}

fn step_selection(state: &mut AppState, traces: &[LoadedTrace], down: bool) {
    let rows = traces
        .get(state.trace_index)
        .map(|trace| entry_rows(&trace.entries))
        .unwrap_or_default();
    let Some(current) = selected_row(&rows, state.selection()) else {
        return;
    };
    let next = if down {
        current + 1
    } else {
        match current.checked_sub(1) {
            Some(previous) => previous,
            None => return,
        }
    };
    if let Some(row) = rows.get(next) {
        state.select(row.selection());
        state.reset_diff_view();
    }
}

pub(super) fn move_entry_down(state: &mut AppState, traces: &[LoadedTrace]) {
    step_selection(state, traces, true);
}

pub(super) fn move_entry_up(state: &mut AppState, traces: &[LoadedTrace]) {
    step_selection(state, traces, false);
}

pub(super) fn render_entries_pane(
    frame: &mut ratatui::Frame<'_>,
    area: ratatui::layout::Rect,
    active_trace: &LoadedTrace,
    state: &mut AppState,
    title: Line<'static>,
    theme: &Theme,
) {
    let rows = entry_rows(&active_trace.entries);
    let selected = selected_row(&rows, state.selection());
    state.entries_list_state.select(selected);
    let entry_items = rows
        .iter()
        .map(|row| ListItem::new(row_label_line(&active_trace.entries, *row, theme)))
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
        rows.len(),
        selected.unwrap_or(0),
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

/// Split a recorded path into its file name and its (display) directory.
fn path_parts(path: &str, cwd: &str) -> (String, String) {
    let path = super::detail::display_path(path, cwd);
    match path.rsplit_once('/') {
        Some((parent, filename)) if !filename.is_empty() => (
            filename.to_string(),
            if parent.is_empty() { "/" } else { parent }.to_string(),
        ),
        _ => (path, String::new()),
    }
}

/// What an entry row names. An edit or write of one file is named by that
/// file; anything else (a command, several files, none) by its reason, with
/// its files listed under it.
enum EntryName {
    File(String, String),
    Reason(String),
}

fn entry_name(entry: &HistoryEntry) -> EntryName {
    match entry.files.as_slice() {
        [file] if entry.command.is_none() => {
            let (filename, parent) = path_parts(&file.path, &entry.cwd);
            EntryName::File(filename, parent)
        }
        _ => EntryName::Reason(entry.reason.clone()),
    }
}

/// Indent of a file row under its entry: past the entry's icon.
const FILE_INDENT: &str = "  ";

fn row_label_line(entries: &[HistoryEntry], row: EntryRow, theme: &Theme) -> Line<'static> {
    let muted = Style::default().fg(rgb_to_color(theme.muted));
    match row {
        EntryRow::Entry(index) => {
            let entry = &entries[index];
            let (icon, icon_color) = entry_icon(entry.ok);
            let mut spans = vec![Span::styled(
                icon.to_string(),
                Style::default().fg(icon_color),
            )];
            match entry_name(entry) {
                EntryName::File(filename, parent) => {
                    spans.push(Span::raw(format!(" {filename}")));
                    if !parent.is_empty() {
                        spans.push(Span::styled(format!("  {parent}"), muted));
                    }
                }
                EntryName::Reason(reason) => spans.push(Span::raw(format!(" {reason}"))),
            }
            Line::from(spans)
        }
        EntryRow::File(index, file) => {
            let entry = &entries[index];
            let (filename, parent) = path_parts(&entry.files[file].path, &entry.cwd);
            let mut spans = vec![Span::raw(format!("{FILE_INDENT}{filename}"))];
            if !parent.is_empty() {
                spans.push(Span::styled(format!("  {parent}"), muted));
            }
            Line::from(spans)
        }
    }
}

/// The plain-text label of `row`, used by the scripted render.
pub(super) fn row_label_plain(entries: &[HistoryEntry], row: EntryRow) -> String {
    match row {
        EntryRow::Entry(index) => {
            let entry = &entries[index];
            let (icon, _) = entry_icon(entry.ok);
            match entry_name(entry) {
                EntryName::File(filename, parent) if parent.is_empty() => {
                    format!("{icon} {filename}")
                }
                EntryName::File(filename, parent) => format!("{icon} {filename}  {parent}"),
                EntryName::Reason(reason) => format!("{icon} {reason}"),
            }
        }
        EntryRow::File(index, file) => {
            let entry = &entries[index];
            let (filename, parent) = path_parts(&entry.files[file].path, &entry.cwd);
            if parent.is_empty() {
                format!("{FILE_INDENT}{filename}")
            } else {
                format!("{FILE_INDENT}{filename}  {parent}")
            }
        }
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
        nested.files[0].path = "/tmp/project/src/main.rs".to_string();
        let mut other = nested.clone();
        other.files[0].path = "/tmp/project/tests/main.rs".to_string();
        other.ok = false;
        let mut outside = nested.clone();
        outside.files[0].path = "/outside/main.rs".to_string();
        let mut relative = nested.clone();
        relative.files[0].path = "lib/main.rs".to_string();
        let mut fallback = nested.clone();
        fallback.files[0].path = "/".to_string();
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
        for (index, label) in labels.iter().enumerate() {
            assert_eq!(
                row_label_plain(&trace.entries, EntryRow::Entry(index)),
                *label
            );
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
            if width != 40 {
                continue;
            }
            for y in [1, 2, 4, 5] {
                assert_eq!(buffer[(12, y)].fg, rgb_to_color(theme.muted));
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

    fn mixed_trace() -> Vec<LoadedTrace> {
        vec![LoadedTrace {
            trace: trace_summary("01JTESTTRACE00000000000000", 2, "a"),
            entries: vec![edit_entry(), two_file_entry()],
        }]
    }

    #[test]
    fn multi_file_entries_list_their_files_under_a_reason_row() {
        let traces = mixed_trace();
        let rows = entry_rows(&traces[0].entries);
        let labels: Vec<String> = rows
            .iter()
            .map(|row| row_label_plain(&traces[0].entries, *row))
            .collect();

        assert_eq!(
            labels,
            [
                "✓ app.txt",
                "✓ Format sources",
                "  a.rs  src",
                "  b.rs  src"
            ]
        );
    }

    #[test]
    fn a_command_entry_shows_its_command_and_lists_even_one_file() {
        let mut command = edit_entry();
        command.command = Some("echo x >> app.txt".to_string());
        command.reason = "echo x >> app.txt".to_string();
        let entries = vec![edit_entry(), command];
        let labels: Vec<String> = entry_rows(&entries)
            .iter()
            .map(|row| row_label_plain(&entries, *row))
            .collect();

        assert_eq!(labels, ["✓ app.txt", "✓ echo x >> app.txt", "  app.txt"]);
    }

    #[test]
    fn j_and_k_walk_entry_and_file_rows() {
        let traces = mixed_trace();
        let mut state = AppState::new(traces.len());

        for _ in 0..3 {
            handle_key(&mut state, &traces, KeyCode::Char('j'), 0, 0);
        }
        assert_eq!(
            state.selection(),
            Selection {
                entry: 1,
                file: Some(1)
            }
        );
        handle_key(&mut state, &traces, KeyCode::Char('j'), 0, 0);
        assert_eq!(state.selection().file, Some(1), "stops at the last row");
        handle_key(&mut state, &traces, KeyCode::Char('k'), 0, 0);
        handle_key(&mut state, &traces, KeyCode::Char('k'), 0, 0);
        assert_eq!(state.selection(), Selection::entry(1));
    }

    #[test]
    fn clicking_a_file_row_selects_that_file() {
        let traces = mixed_trace();
        let mut state = state_with_rects(&traces);

        // Row 4 is the fourth list row: the second file of entry 1.
        let mouse = make_mouse(MouseEventKind::Down(MouseButton::Left), 5, 4);
        handle_mouse(&mut state, &traces, mouse, 20, 10);

        assert_eq!(
            state.selection(),
            Selection {
                entry: 1,
                file: Some(1)
            }
        );
    }
}

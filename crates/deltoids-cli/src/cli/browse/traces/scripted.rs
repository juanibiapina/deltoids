//! Headless render path: when stdout is not a terminal, drive the same
//! state from a scripted key string and print a plain-text snapshot.
//! Used by tests and non-interactive callers.

use std::io::{self, Read};

use deltoids::Theme;
use deltoids::render_tui::position_footer;

use crate::sidebar_width;

use super::detail::{fit_line, render_detail_for};
use super::entries_pane::entry_label_plain;
use super::model::LoadedTrace;
use super::traces_pane::trace_label;
use super::{AppState, DIFF_SCROLL_STEP, Focus, move_down, move_up};

pub(super) fn run_scripted(traces: &[LoadedTrace], theme: &Theme) -> Result<(), String> {
    let mut state = AppState::new(traces.len());
    let mut input = String::new();
    io::stdin()
        .read_to_string(&mut input)
        .map_err(|err| format!("Failed to read stdin: {err}"))?;

    for ch in input.chars() {
        match ch {
            'j' => {
                if state.focus == Focus::Diff {
                    state.diff_scroll += DIFF_SCROLL_STEP;
                } else {
                    move_down(&mut state, traces);
                }
            }
            'k' => {
                if state.focus == Focus::Diff {
                    state.diff_scroll = state.diff_scroll.saturating_sub(DIFF_SCROLL_STEP);
                } else {
                    move_up(&mut state, traces);
                }
            }
            '\t' => {
                state.focus = match state.focus {
                    Focus::Entries => Focus::Traces,
                    Focus::Traces => Focus::Diff,
                    Focus::Diff => Focus::Entries,
                };
            }
            'J' => state.diff_scroll += DIFF_SCROLL_STEP,
            'K' => state.diff_scroll = state.diff_scroll.saturating_sub(DIFF_SCROLL_STEP),
            '1' => state.focus = Focus::Entries,
            '2' => state.focus = Focus::Traces,
            '3' => state.focus = Focus::Diff,
            'q' => break,
            _ => {}
        }
    }

    print!("{}", render_scripted(traces, &state, 120, 30, theme));
    Ok(())
}

fn render_scripted(
    traces: &[LoadedTrace],
    state: &AppState,
    width: usize,
    height: usize,
    theme: &Theme,
) -> String {
    if traces.is_empty() {
        return "No traces found for this directory.\n".to_string();
    }

    let left_width = sidebar_width::default_width(width as u16) as usize;
    let right_width = width.saturating_sub(left_width + 3);
    let body_height = height.max(3);
    let sidebar_half = (body_height / 2).max(2);

    let active_trace = &traces[state.trace_index];

    // Top-left: entries list (header + entries, padded/truncated to sidebar_half rows)
    let focus_entries_marker = if state.focus == Focus::Entries {
        "*"
    } else {
        " "
    };
    let entries_count = active_trace.entries.len();
    let entries_position = if entries_count == 0 {
        0
    } else {
        state.entry_index() + 1
    };
    let mut entries_section = vec![format!(
        "{focus_entries_marker} [1] Entries {}",
        position_footer(entries_position, entries_count).trim()
    )];
    for (index, entry) in active_trace.entries.iter().enumerate() {
        let marker = if index == state.entry_index() {
            ">"
        } else {
            " "
        };
        entries_section.push(fit_line(
            &format!("{marker} {}", entry_label_plain(entry)),
            left_width,
        ));
    }

    // Bottom-left: traces list
    let focus_traces_marker = if state.focus == Focus::Traces {
        "*"
    } else {
        " "
    };
    let traces_count = traces.len();
    let traces_position = if traces_count == 0 {
        0
    } else {
        state.trace_index + 1
    };
    let mut traces_section = vec![format!(
        "{focus_traces_marker} [2] Traces {}",
        position_footer(traces_position, traces_count).trim()
    )];
    for (index, loaded) in traces.iter().enumerate() {
        let marker = if index == state.trace_index { ">" } else { " " };
        traces_section.push(fit_line(
            &format!("{marker} {}", trace_label(&loaded.trace)),
            left_width,
        ));
    }

    let entries_rows = pad_or_truncate(&entries_section, sidebar_half);
    let traces_rows = pad_or_truncate(&traces_section, body_height.saturating_sub(sidebar_half));
    let sidebar_rows = [entries_rows, traces_rows].concat();

    // Right: diff for selected entry, spans full body height
    let detail = render_detail_for(active_trace, state.entry_index(), right_width, theme)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();
    let diff_rows = detail
        .iter()
        .skip(state.diff_scroll)
        .take(body_height)
        .map(|line| fit_line(line, right_width))
        .collect::<Vec<_>>();

    let mut output = String::new();
    for row in 0..body_height {
        let left = sidebar_rows.get(row).map(String::as_str).unwrap_or("");
        let right = diff_rows.get(row).map(String::as_str).unwrap_or("");
        output.push_str(&format!("{left:<left_width$} | {right}\n"));
    }

    output
}

fn pad_or_truncate(rows: &[String], target: usize) -> Vec<String> {
    let mut result = rows.iter().take(target).cloned().collect::<Vec<_>>();
    while result.len() < target {
        result.push(String::new());
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::browse::traces::test_support::*;

    #[test]
    fn scripted_render_shows_traces_and_entries() {
        let traces = vec![
            LoadedTrace {
                trace: trace_summary("01JTESTTRACE00000000000000", 2, "Update x"),
                entries: vec![edit_entry(), write_entry()],
            },
            LoadedTrace {
                trace: trace_summary("01JTESTTRACE00000000000001", 1, "other"),
                entries: vec![edit_entry()],
            },
        ];
        let state = AppState::new(traces.len());

        let theme = test_theme();
        let output = render_scripted(&traces, &state, 140, 30, &theme);

        // Entries list shows each entry's reason.
        assert!(output.contains("\u{2713} Update x constant"));
        assert!(output.contains("\u{2713} Rewrite config"));
        assert!(output.contains("01JTESTTRA"));
        assert!(output.contains("[1] Entries 1 of 2"));
        assert!(output.contains("[2] Traces 1 of 2"));
        // Detail header shows the selected entry's path.
        assert!(output.contains("app.txt"));
        // v1 entries show deprecation message instead of diff content
        assert!(output.contains("(old format, cannot display)"));
    }

    #[test]
    fn scripted_render_shows_empty_message() {
        let state = AppState::new(0);
        let theme = test_theme();
        let output = render_scripted(&[], &state, 140, 30, &theme);
        assert!(output.contains("No traces"));
    }

    #[test]
    fn scripted_selection_updates_after_navigation() {
        let traces = vec![LoadedTrace {
            trace: trace_summary("01JTESTTRACE00000000000000", 2, "Update x"),
            entries: vec![edit_entry(), write_entry()],
        }];
        let mut state = AppState::new(traces.len());
        state.focus = Focus::Entries;
        move_down(&mut state, &traces);

        let theme = test_theme();
        let output = render_scripted(&traces, &state, 140, 30, &theme);

        assert!(output.contains("> \u{2713} Rewrite config"));
    }
}

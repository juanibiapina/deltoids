//! Headless render path: when stdout is not a terminal, drive the same
//! state from a scripted key string and print a plain-text snapshot.
//! Used by tests and non-interactive callers.

use std::io::{self, Read};

use crossterm::event::KeyCode;

use deltoids::Theme;
use deltoids::render_tui::position_footer;

use crate::cli::browse::mode::AppCommand;

use crate::sidebar_width;

use super::detail::{CacheEpoch, build_diff_rows, ensure_diff_cache, fit_line};
use super::entries_pane::entry_label_plain;
use super::model::LoadedTrace;
use super::traces_pane::trace_label;
use crate::cli::browse::comment_view::with_comments;
use crate::cli::browse::diff_cursor::Step;

use super::{
    AppState, DIFF_SCROLL_STEP, Focus, InputState, copy_comments, delete_comment,
    handle_comment_key, move_diff_cursor, move_down, move_up, open_comment,
};

const SCRIPTED_WIDTH: usize = 120;
const SCRIPTED_HEIGHT: usize = 30;

/// Diff-pane width the scripted renderer uses, matching the geometry in
/// [`render_scripted`] so cursor targets line up with what is printed.
fn scripted_right_width() -> usize {
    let left_width = sidebar_width::default_width(SCRIPTED_WIDTH as u16) as usize;
    SCRIPTED_WIDTH.saturating_sub(left_width + 3)
}

pub(super) fn run_scripted(traces: &[LoadedTrace], theme: &Theme) -> Result<(), String> {
    let mut state = AppState::new(traces.len());
    let mut input = String::new();
    io::stdin()
        .read_to_string(&mut input)
        .map_err(|err| format!("Failed to read stdin: {err}"))?;

    let right_width = scripted_right_width();
    // Keys that act on diff rows need the rows to exist first; the
    // interactive path gets that from the previous draw.
    let ensure_rows = |state: &mut AppState| {
        if let Some(active) = traces.get(state.trace_index) {
            let key = state.diff_key();
            let epoch = CacheEpoch {
                width: right_width,
                layout: deltoids::ChangeLayout::Grouped,
            };
            ensure_diff_cache(active, state, epoch, key, theme);
        }
    };

    // Text typed into the comment editor is echoed nowhere; the printed
    // snapshot and any copied prompt are the observable result.
    let mut copied: Option<String> = None;

    for ch in input.chars() {
        if matches!(state.input, InputState::Commenting { .. }) {
            let key = match ch {
                '\n' | '\r' => KeyCode::Enter,
                '\u{1b}' => KeyCode::Esc,
                '\u{8}' | '\u{7f}' => KeyCode::Backspace,
                other => KeyCode::Char(other),
            };
            handle_comment_key(&mut state, key);
            continue;
        }

        match ch {
            'j' => {
                if state.focus == Focus::Diff {
                    ensure_rows(&mut state);
                    move_diff_cursor(&mut state, Step::Down, SCRIPTED_HEIGHT);
                } else {
                    move_down(&mut state, traces);
                }
            }
            'k' => {
                if state.focus == Focus::Diff {
                    ensure_rows(&mut state);
                    move_diff_cursor(&mut state, Step::Up, SCRIPTED_HEIGHT);
                } else {
                    move_up(&mut state, traces);
                }
            }
            'c' => {
                ensure_rows(&mut state);
                open_comment(&mut state, traces);
            }
            'd' => {
                ensure_rows(&mut state);
                delete_comment(&mut state);
            }
            'y' => {
                if let AppCommand::CopyToClipboard(prompt) = copy_comments(&mut state, traces) {
                    copied = Some(prompt);
                }
            }
            '\t' => {
                state.focus = match state.focus {
                    Focus::Entries => Focus::Traces,
                    Focus::Traces => Focus::Diff,
                    Focus::Diff => Focus::Entries,
                };
            }
            'J' => state.cursor.scroll += DIFF_SCROLL_STEP,
            'K' => state.cursor.scroll = state.cursor.scroll.saturating_sub(DIFF_SCROLL_STEP),
            '1' => state.focus = Focus::Entries,
            '2' => state.focus = Focus::Traces,
            '3' => state.focus = Focus::Diff,
            'q' => break,
            _ => {}
        }
    }

    // Without a terminal there is no clipboard to write to, so the prompt
    // goes to stdout: the scripted path's observable copy result.
    if let Some(prompt) = copied {
        print!("{prompt}");
    }
    print!(
        "{}",
        render_scripted(traces, &state, SCRIPTED_WIDTH, SCRIPTED_HEIGHT, theme)
    );
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
    let rows = build_diff_rows(
        active_trace,
        state.entry_index(),
        right_width,
        deltoids::ChangeLayout::Grouped,
        theme,
    );
    let detail = with_comments(&rows, &state.comments, right_width, theme, |_, _| false)
        .into_iter()
        .map(|row| row.line.to_string())
        .collect::<Vec<_>>();
    let diff_rows = detail
        .iter()
        .skip(state.cursor.scroll)
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

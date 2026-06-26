//! Shared test fixtures for the traces TUI submodules: theme, history
//! entry/trace builders, event builders, and a state-with-rects helper.

use crossterm::event::{MouseEvent, MouseEventKind};
use ratatui::layout::Rect;

use deltoids::Theme;

use crate::{HistoryEntry, TextEdit, TraceSummary};

use super::AppState;
use super::model::LoadedTrace;

pub(super) fn test_theme() -> Theme {
    Theme::load()
}

pub(super) fn edit_entry() -> HistoryEntry {
    HistoryEntry {
        v: 1,
        tool: "edit".to_string(),
        trace_id: "01JTESTTRACE00000000000000".to_string(),
        timestamp: "2026-04-16T12:00:00Z".to_string(),
        cwd: "/tmp/project".to_string(),
        path: "/tmp/project/app.txt".to_string(),
        reason: "Update x constant".to_string(),
        ok: true,
        edits: vec![TextEdit {
            reason: "Edit change".to_string(),
            old_text: "const x = 1;".to_string(),
            new_text: "const x = 2;".to_string(),
        }],
        content: String::new(),
        diff: Some(
            "--- a/app.txt\n+++ b/app.txt\n@@ -1 +1 @@ fn update() {\n-const x = 1;\n+const x = 2;\n"
                .to_string(),
        ),
        error: None,
        hunks: Vec::new(),
        language: None,
        highlight: None,
    }
}

pub(super) fn write_entry() -> HistoryEntry {
    HistoryEntry {
        v: 1,
        tool: "write".to_string(),
        trace_id: "01JTESTTRACE00000000000000".to_string(),
        timestamp: "2026-04-16T12:01:00Z".to_string(),
        cwd: "/tmp/project".to_string(),
        path: "/tmp/project/config.json".to_string(),
        reason: "Rewrite config".to_string(),
        ok: true,
        edits: Vec::new(),
        content: "{\n  \"version\": 2\n}\n".to_string(),
        diff: Some(
            "--- a/config.json\n+++ b/config.json\n@@ -1,3 +1,3 @@\n   \"version\": 1\n+  \"version\": 2\n"
                .to_string(),
        ),
        error: None,
        hunks: Vec::new(),
        language: None,
        highlight: None,
    }
}

pub(super) fn trace_summary(trace_id: &str, entry_count: usize, last_reason: &str) -> TraceSummary {
    TraceSummary {
        trace_id: trace_id.to_string(),
        entry_count,
        last_timestamp: "2026-04-16T12:00:00Z".to_string(),
        last_tool: "edit".to_string(),
        last_path: "/tmp/project/app.txt".to_string(),
        last_reason: last_reason.to_string(),
    }
}

pub(super) fn make_mouse(kind: MouseEventKind, col: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind,
        column: col,
        row,
        modifiers: crossterm::event::KeyModifiers::NONE,
    }
}

pub(super) fn state_with_rects(traces: &[LoadedTrace]) -> AppState {
    let mut state = AppState::new(traces.len());
    state.entries_rect = Rect::new(0, 0, 30, 10);
    state.traces_rect = Rect::new(0, 10, 30, 10);
    state.diff_rect = Rect::new(30, 0, 90, 20);
    state
}

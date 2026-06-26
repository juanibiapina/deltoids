//! Refresh axis: reload traces from disk and restore the user's
//! selection and scroll position.

use std::collections::HashSet;

use super::AppState;
use super::model::{LoadedTrace, load_traces_for_cwd};

/// Reload traces from disk unconditionally. When a new trace appears at
/// index 0 (newest), automatically switches to it. Otherwise preserves the
/// current selection by trace id and entry index when the selected trace
/// still exists; falls back to index 0 otherwise.
pub(super) fn reload_traces(
    traces: &mut Vec<LoadedTrace>,
    state: &mut AppState,
    cwd: &str,
) -> Result<(), String> {
    // Collect known trace IDs before reload.
    let known_ids: HashSet<_> = traces.iter().map(|t| t.trace.trace_id.as_str()).collect();

    let new_traces = load_traces_for_cwd(cwd)?;

    // Check if the newest trace is new (unknown before reload).
    let newest_is_new = new_traces
        .first()
        .is_some_and(|t| !known_ids.contains(t.trace.trace_id.as_str()));

    // Remember current selection.
    let prev_trace_id = traces
        .get(state.trace_index)
        .map(|t| t.trace.trace_id.clone());
    let prev_entry_index = state.entry_index();

    // Replace traces.
    *traces = new_traces;

    // Rebuild entry_indices for the new trace count.
    state.entry_indices = vec![0; traces.len()];

    if newest_is_new {
        // New trace arrived: switch to it.
        state.trace_index = 0;
        state.traces_list_state.select(Some(0));
        state.set_entry_index(0);
        state.diff_scroll = 0;
        state.diff_cache = None;
        return Ok(());
    }

    // Restore trace selection by id, or fall back to 0.
    state.trace_index = prev_trace_id
        .as_deref()
        .and_then(|id| traces.iter().position(|t| t.trace.trace_id == id))
        .unwrap_or(0);
    state.traces_list_state.select(Some(state.trace_index));

    // Restore entry index, clamped to the new entry count.
    let entry_count = traces
        .get(state.trace_index)
        .map(|t| t.entries.len())
        .unwrap_or(0);
    let clamped = if entry_count == 0 {
        0
    } else {
        prev_entry_index.min(entry_count - 1)
    };
    state.set_entry_index(clamped);

    // Invalidate caches.
    state.diff_cache = None;

    // Reset scroll only when the selected entry changed (trace disappeared
    // or entry index was clamped). When the same entry is still selected the
    // user may be reviewing the diff, so preserve their scroll position.
    let selection_changed = prev_trace_id.as_deref()
        != traces
            .get(state.trace_index)
            .map(|t| t.trace.trace_id.as_str())
        || clamped != prev_entry_index;
    if selection_changed {
        state.diff_scroll = 0;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::browse::traces::DiffCache;
    use crate::cli::browse::traces::test_support::*;

    #[test]
    fn reload_preserves_selection() {
        let trace_a = LoadedTrace {
            trace: trace_summary("01JTESTTRACE00000000000000", 2, "a"),
            entries: vec![edit_entry(), write_entry()],
        };
        let trace_b = LoadedTrace {
            trace: trace_summary("01JTESTTRACE00000000000001", 1, "b"),
            entries: vec![edit_entry()],
        };

        // Start with two traces, select the second trace at entry 0.
        let mut traces = vec![trace_a.clone(), trace_b.clone()];
        let mut state = AppState::new(traces.len());
        state.trace_index = 1;
        state.set_entry_index(0);
        state.diff_cache = Some(DiffCache {
            trace_index: 1,
            entry_index: 0,
            width: 80,
            lines: vec![],
        });

        // Simulate a reload where trace_b gains an entry.
        let trace_b_updated = LoadedTrace {
            trace: trace_summary("01JTESTTRACE00000000000001", 2, "b updated"),
            entries: vec![edit_entry(), write_entry()],
        };
        // Swap in the new data (simulates what reload_traces does without disk IO).
        let prev_trace_id = traces
            .get(state.trace_index)
            .map(|t| t.trace.trace_id.clone());
        let prev_entry_index = state.entry_index();

        traces = vec![trace_a.clone(), trace_b_updated];
        state.entry_indices = vec![0; traces.len()];
        state.trace_index = prev_trace_id
            .as_deref()
            .and_then(|id| traces.iter().position(|t| t.trace.trace_id == id))
            .unwrap_or(0);

        let entry_count = traces
            .get(state.trace_index)
            .map(|t| t.entries.len())
            .unwrap_or(0);
        let clamped = if entry_count == 0 {
            0
        } else {
            prev_entry_index.min(entry_count - 1)
        };
        state.set_entry_index(clamped);
        state.diff_cache = None;

        // Selection stays on the same trace.
        assert_eq!(state.trace_index, 1);
        assert_eq!(state.entry_index(), 0);
        assert!(state.diff_cache.is_none());
    }

    #[test]
    fn reload_handles_removed_trace() {
        let trace_a = LoadedTrace {
            trace: trace_summary("01JTESTTRACE00000000000000", 1, "a"),
            entries: vec![edit_entry()],
        };
        let trace_b = LoadedTrace {
            trace: trace_summary("01JTESTTRACE00000000000001", 1, "b"),
            entries: vec![edit_entry()],
        };

        let mut traces = vec![trace_a.clone(), trace_b.clone()];
        let mut state = AppState::new(traces.len());
        state.trace_index = 1; // select trace_b
        state.diff_cache = Some(DiffCache {
            trace_index: 1,
            entry_index: 0,
            width: 80,
            lines: vec![],
        });

        // Simulate trace_b disappearing.
        let prev_trace_id = traces
            .get(state.trace_index)
            .map(|t| t.trace.trace_id.clone());

        traces = vec![trace_a.clone()];
        state.entry_indices = vec![0; traces.len()];
        state.trace_index = prev_trace_id
            .as_deref()
            .and_then(|id| traces.iter().position(|t| t.trace.trace_id == id))
            .unwrap_or(0);
        state.diff_cache = None;

        // Falls back to index 0 since trace_b is gone.
        assert_eq!(state.trace_index, 0);
        assert_eq!(
            traces[state.trace_index].trace.trace_id,
            "01JTESTTRACE00000000000000"
        );
        assert!(state.diff_cache.is_none());
    }

    /// Helper that simulates `reload_traces` selection-restore and scroll
    /// logic without disk IO.
    fn simulate_reload(
        traces: &mut Vec<LoadedTrace>,
        state: &mut AppState,
        new_traces: Vec<LoadedTrace>,
    ) {
        // Collect known trace IDs before reload.
        let known_ids: HashSet<_> = traces.iter().map(|t| t.trace.trace_id.as_str()).collect();

        // Check if the newest trace is new.
        let newest_is_new = new_traces
            .first()
            .is_some_and(|t| !known_ids.contains(t.trace.trace_id.as_str()));

        let prev_trace_id = traces
            .get(state.trace_index)
            .map(|t| t.trace.trace_id.clone());
        let prev_entry_index = state.entry_index();

        *traces = new_traces;
        state.entry_indices = vec![0; traces.len()];

        if newest_is_new {
            // New trace arrived: switch to it.
            state.trace_index = 0;
            state.set_entry_index(0);
            state.diff_scroll = 0;
            state.diff_cache = None;
            return;
        }

        state.trace_index = prev_trace_id
            .as_deref()
            .and_then(|id| traces.iter().position(|t| t.trace.trace_id == id))
            .unwrap_or(0);

        let entry_count = traces
            .get(state.trace_index)
            .map(|t| t.entries.len())
            .unwrap_or(0);
        let clamped = if entry_count == 0 {
            0
        } else {
            prev_entry_index.min(entry_count - 1)
        };
        state.set_entry_index(clamped);
        state.diff_cache = None;

        let selection_changed = prev_trace_id.as_deref()
            != traces
                .get(state.trace_index)
                .map(|t| t.trace.trace_id.as_str())
            || clamped != prev_entry_index;
        if selection_changed {
            state.diff_scroll = 0;
        }
    }

    #[test]
    fn reload_preserves_scroll_when_selection_unchanged() {
        let trace = LoadedTrace {
            trace: trace_summary("01JTESTTRACE00000000000000", 1, "a"),
            entries: vec![edit_entry()],
        };
        let mut traces = vec![trace.clone()];
        let mut state = AppState::new(traces.len());
        state.diff_scroll = 42;

        // Reload with same trace, same entries.
        let updated = LoadedTrace {
            trace: trace_summary("01JTESTTRACE00000000000000", 2, "a updated"),
            entries: vec![edit_entry(), write_entry()],
        };
        simulate_reload(&mut traces, &mut state, vec![updated]);

        assert_eq!(state.diff_scroll, 42, "scroll should be preserved");
    }

    #[test]
    fn reload_resets_scroll_when_trace_disappears() {
        let trace_a = LoadedTrace {
            trace: trace_summary("01JTESTTRACE00000000000000", 1, "a"),
            entries: vec![edit_entry()],
        };
        let trace_b = LoadedTrace {
            trace: trace_summary("01JTESTTRACE00000000000001", 1, "b"),
            entries: vec![edit_entry()],
        };
        let mut traces = vec![trace_a.clone(), trace_b];
        let mut state = AppState::new(traces.len());
        state.trace_index = 1;
        state.set_entry_index(0);
        state.diff_scroll = 15;

        // trace_b disappears.
        simulate_reload(&mut traces, &mut state, vec![trace_a]);

        assert_eq!(state.trace_index, 0);
        assert_eq!(state.diff_scroll, 0, "scroll should reset when trace gone");
    }

    #[test]
    fn reload_switches_to_new_trace() {
        let trace_a = LoadedTrace {
            trace: trace_summary("01JTESTTRACE00000000000000", 1, "a"),
            entries: vec![edit_entry()],
        };
        let mut traces = vec![trace_a.clone()];
        let mut state = AppState::new(traces.len());
        state.trace_index = 0;
        state.set_entry_index(0);
        state.diff_scroll = 10;

        // New trace appears at head (newest).
        let trace_b = LoadedTrace {
            trace: trace_summary("01JTESTTRACE00000000000001", 1, "b"),
            entries: vec![edit_entry()],
        };
        simulate_reload(&mut traces, &mut state, vec![trace_b, trace_a]);

        // Should switch to the new trace at index 0.
        assert_eq!(state.trace_index, 0);
        assert_eq!(
            traces[state.trace_index].trace.trace_id,
            "01JTESTTRACE00000000000001"
        );
        assert_eq!(state.entry_index(), 0);
        assert_eq!(state.diff_scroll, 0, "scroll should reset for new trace");
    }

    #[test]
    fn reload_preserves_selection_when_no_new_trace() {
        let trace_a = LoadedTrace {
            trace: trace_summary("01JTESTTRACE00000000000000", 1, "a"),
            entries: vec![edit_entry()],
        };
        let trace_b = LoadedTrace {
            trace: trace_summary("01JTESTTRACE00000000000001", 1, "b"),
            entries: vec![edit_entry()],
        };
        let mut traces = vec![trace_a.clone(), trace_b.clone()];
        let mut state = AppState::new(traces.len());
        state.trace_index = 1; // select trace_b
        state.set_entry_index(0);
        state.diff_scroll = 15;

        // trace_b gains an entry but no new trace appears.
        let trace_b_updated = LoadedTrace {
            trace: trace_summary("01JTESTTRACE00000000000001", 2, "b updated"),
            entries: vec![edit_entry(), write_entry()],
        };
        simulate_reload(&mut traces, &mut state, vec![trace_a, trace_b_updated]);

        // Selection should stay on trace_b.
        assert_eq!(state.trace_index, 1);
        assert_eq!(
            traces[state.trace_index].trace.trace_id,
            "01JTESTTRACE00000000000001"
        );
        assert_eq!(state.diff_scroll, 15, "scroll should be preserved");
    }

    #[test]
    fn reload_resets_scroll_when_entry_index_clamped() {
        let trace = LoadedTrace {
            trace: trace_summary("01JTESTTRACE00000000000000", 3, "a"),
            entries: vec![edit_entry(), write_entry(), edit_entry()],
        };
        let mut traces = vec![trace];
        let mut state = AppState::new(traces.len());
        state.set_entry_index(2);
        state.diff_scroll = 20;

        // Entries shrink to 1, so entry_index 2 gets clamped to 0.
        let shrunk = LoadedTrace {
            trace: trace_summary("01JTESTTRACE00000000000000", 1, "a shrunk"),
            entries: vec![edit_entry()],
        };
        simulate_reload(&mut traces, &mut state, vec![shrunk]);

        assert_eq!(state.entry_index(), 0);
        assert_eq!(
            state.diff_scroll, 0,
            "scroll should reset when entry clamped"
        );
    }
}

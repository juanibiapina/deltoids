//! Apply a project snapshot while retaining review selection and rendered rows.

use std::collections::{HashMap, HashSet};

use super::model::LoadedTrace;
use super::reader::Refresh;
use super::{AppState, Selection};

pub(super) fn apply_refresh(traces: &mut Vec<LoadedTrace>, state: &mut AppState, refresh: Refresh) {
    let known: HashSet<_> = traces
        .iter()
        .map(|trace| trace.trace.trace_id.as_str())
        .collect();
    let newest_is_new = refresh
        .traces
        .first()
        .is_some_and(|trace| !known.contains(trace.trace.trace_id.as_str()));
    let previous_id = traces
        .get(state.trace_index)
        .map(|trace| trace.trace.trace_id.clone());
    let previous_selection = state.selection();
    let previous_entry = previous_selection.entry;
    let selections: HashMap<_, _> = traces
        .iter()
        .enumerate()
        .map(|(index, trace)| {
            (
                trace.trace.trace_id.clone(),
                state.selections.get(index).copied().unwrap_or_default(),
            )
        })
        .collect();
    let source_changed = previous_id.as_ref().is_some_and(|id| {
        refresh
            .retained
            .iter()
            .find(|(trace, _)| trace == id)
            .is_none_or(|(_, prefix)| previous_entry >= *prefix)
    });
    state.diff_cache.retain(&refresh.retained);
    *traces = refresh.traces;
    state.selections = traces
        .iter()
        .map(|trace| {
            let selection = selections
                .get(&trace.trace.trace_id)
                .copied()
                .unwrap_or_default();
            let entry = selection.entry.min(trace.entries.len().saturating_sub(1));
            // A file row survives only on the same, still-present entry.
            let file = selection.file.filter(|file| {
                entry == selection.entry
                    && trace
                        .entries
                        .get(entry)
                        .is_some_and(|loaded| *file < loaded.files.len())
            });
            Selection { entry, file }
        })
        .collect();
    state.trace_index = if newest_is_new {
        0
    } else {
        previous_id
            .as_ref()
            .and_then(|id| traces.iter().position(|trace| &trace.trace.trace_id == id))
            .unwrap_or(0)
    };
    state
        .traces_list_state
        .select((!traces.is_empty()).then_some(state.trace_index));
    if newest_is_new {
        state.set_entry_index(0);
    }
    let selected_id = traces
        .get(state.trace_index)
        .map(|trace| &trace.trace.trace_id);
    if newest_is_new
        || selected_id != previous_id.as_ref()
        || state.selection() != previous_selection
        || source_changed
    {
        state.reset_diff_view();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::browse::diff_cursor::DiffRow;
    use crate::cli::browse::traces::detail::CacheEpoch;
    use crate::cli::browse::traces::test_support::*;
    use ratatui::text::Line;

    fn trace(id: &str, count: usize) -> LoadedTrace {
        LoadedTrace {
            trace: trace_summary(id, count, id),
            entries: vec![edit_entry(); count],
        }
    }

    fn refresh(traces: Vec<LoadedTrace>) -> Refresh {
        let retained = traces
            .iter()
            .map(|trace| (trace.trace.trace_id.clone(), trace.entries.len()))
            .collect();
        Refresh { traces, retained }
    }

    #[test]
    fn appending_an_entry_keeps_the_selected_file_row() {
        let mut traces = vec![LoadedTrace {
            trace: trace_summary("a", 1, "a"),
            entries: vec![two_file_entry()],
        }];
        let mut state = AppState::new(1);
        let selected = Selection {
            entry: 0,
            file: Some(1),
        };
        state.select(selected);
        let mut grown = traces.clone();
        grown[0].entries.push(edit_entry());

        apply_refresh(&mut traces, &mut state, refresh(grown));

        assert_eq!(state.selection(), selected);
    }

    #[test]
    fn reordering_preserves_selection_scroll_and_cached_identity() {
        let mut traces = vec![trace("a", 2), trace("b", 1)];
        let mut state = AppState::new(2);
        state.set_entry_index(1);
        state.cursor.scroll = 42;
        let epoch = CacheEpoch::default();
        state.diff_cache.insert(
            epoch,
            ("a", Selection::entry(1)),
            vec![DiffRow::plain(Line::from("source a"))],
        );
        apply_refresh(
            &mut traces,
            &mut state,
            refresh(vec![trace("b", 2), trace("a", 2)]),
        );
        assert_eq!(traces[state.trace_index].trace.trace_id, "a");
        assert_eq!(state.entry_index(), 1);
        assert_eq!(state.cursor.scroll, 42);
        assert_eq!(
            state
                .diff_cache
                .get(epoch, ("a", Selection::entry(1)))
                .unwrap()[0]
                .line,
            Line::from("source a")
        );
        assert!(!state.diff_cache.contains(epoch, ("b", Selection::entry(1))));
    }

    #[test]
    fn new_trace_becomes_selected_without_evicting_older_renders() {
        let mut traces = vec![trace("a", 1)];
        let mut state = AppState::new(1);
        state.cursor.scroll = 12;
        state
            .diff_cache
            .insert(CacheEpoch::default(), ("a", Selection::entry(0)), vec![]);
        apply_refresh(
            &mut traces,
            &mut state,
            refresh(vec![trace("new", 1), trace("a", 1)]),
        );
        assert_eq!(traces[state.trace_index].trace.trace_id, "new");
        assert_eq!(state.cursor.scroll, 0);
        assert!(
            state
                .diff_cache
                .contains(CacheEpoch::default(), ("a", Selection::entry(0)))
        );
    }

    #[test]
    fn deletion_and_truncation_clamp_selection_and_remove_stale_rows() {
        let mut traces = vec![trace("a", 3), trace("b", 1)];
        let mut state = AppState::new(2);
        state.set_entry_index(2);
        state.cursor.scroll = 20;
        let epoch = CacheEpoch::default();
        state
            .diff_cache
            .insert(epoch, ("a", Selection::entry(2)), vec![]);
        state
            .diff_cache
            .insert(epoch, ("b", Selection::entry(0)), vec![]);
        apply_refresh(&mut traces, &mut state, refresh(vec![trace("a", 1)]));
        assert_eq!(state.entry_index(), 0);
        assert_eq!(state.cursor.scroll, 0);
        assert!(state.diff_cache.is_empty());
        apply_refresh(&mut traces, &mut state, refresh(vec![]));
        assert!(traces.is_empty());
        assert_eq!(state.trace_index, 0);
    }

    #[test]
    fn rewrite_invalidates_only_the_changed_suffix() {
        let mut traces = vec![trace("a", 2), trace("b", 1)];
        let mut state = AppState::new(2);
        state.set_entry_index(1);
        state.cursor.scroll = 20;
        let epoch = CacheEpoch::default();
        for key in [
            ("a", Selection::entry(0)),
            ("a", Selection::entry(1)),
            ("b", Selection::entry(0)),
        ] {
            state.diff_cache.insert(epoch, key, vec![]);
        }
        let mut update = refresh(traces.clone());
        update.retained[0].1 = 1;
        apply_refresh(&mut traces, &mut state, update);
        assert!(state.diff_cache.contains(epoch, ("a", Selection::entry(0))));
        assert!(state.diff_cache.contains(epoch, ("b", Selection::entry(0))));
        assert!(!state.diff_cache.contains(epoch, ("a", Selection::entry(1))));
        assert_eq!(state.cursor.scroll, 0);
    }
}

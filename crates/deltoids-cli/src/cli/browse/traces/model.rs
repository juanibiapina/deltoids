//! Data axis for the traces TUI: load the traces for the current
//! directory and the entries that belong to it.

use crate::{HistoryEntry, TraceSummary, list_traces_for_current_directory, read_history_entries};

/// One trace plus the subset of its entries that belong to the current
/// working directory.
#[derive(Debug, Clone)]
pub(super) struct LoadedTrace {
    pub(super) trace: TraceSummary,
    pub(super) entries: Vec<HistoryEntry>,
}

pub(super) fn load_traces_for_cwd(cwd: &str) -> Result<Vec<LoadedTrace>, String> {
    let traces = list_traces_for_current_directory()?;
    let mut loaded = Vec::with_capacity(traces.len());
    for trace in traces {
        let entries = read_history_entries(&trace.trace_id)?
            .into_iter()
            .filter(|entry| entry.cwd == cwd)
            .collect::<Vec<_>>();
        loaded.push(LoadedTrace { trace, entries });
    }
    Ok(loaded)
}

pub(super) fn current_cwd_or_empty() -> String {
    std::env::current_dir()
        .ok()
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_default()
}

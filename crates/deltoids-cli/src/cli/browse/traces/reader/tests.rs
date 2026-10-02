use super::*;
use serde_json::{Value, json};
use std::io::Write;

fn entry(cwd: &str, number: usize) -> Value {
    json!({"v": 1, "tool": "write", "traceId": "probe", "timestamp": format!("2026-10-01T00:00:{number:02}Z"), "cwd": cwd, "path": "file.txt", "summary": format!("entry {number} café"), "ok": true, "content": "recorded content"})
}

fn setup() -> (tempfile::TempDir, TraceStore, TraceReader) {
    let dir = tempfile::tempdir().unwrap();
    let store = TraceStore::with_root(dir.path().to_path_buf());
    let reader = TraceReader::new(store.clone(), "local".to_owned());
    (dir, store, reader)
}

fn path(store: &TraceStore, id: &str) -> PathBuf {
    store.trace_directory(id).join("entries.jsonl")
}

fn changed(reader: &mut TraceReader, store: &TraceStore, id: &str) {
    assert!(reader.notify(&[path(store, id)], false));
}

#[test]
fn unrelated_append_does_not_read_or_rebuild_local_histories() {
    let (_dir, store, mut reader) = setup();
    store.append("local", &entry("local", 1)).unwrap();
    store.append("other", &entry("other", 2)).unwrap();
    assert_eq!(reader.refresh().unwrap().unwrap().traces.len(), 1);
    // An unrelated event must not touch this now-unreadable local history.
    fs::write(path(&store, "local"), b"malformed history\n").unwrap();
    store.append("other", &entry("other", 3)).unwrap();
    changed(&mut reader, &store, "other");
    assert!(reader.refresh().unwrap().is_none());
    assert!(!reader.needs_retry());
    assert!(reader.refresh().unwrap().is_none());
}

#[test]
fn mixed_project_append_preserves_earlier_local_entries_and_reorders() {
    let (_dir, store, mut reader) = setup();
    store.append("a", &entry("local", 1)).unwrap();
    store.append("a", &entry("other", 2)).unwrap();
    store.append("b", &entry("local", 3)).unwrap();
    let first = reader.refresh().unwrap().unwrap();
    assert_eq!(first.traces[0].trace.trace_id, "b");
    assert_eq!(first.traces[1].entries.len(), 1);
    store.append("a", &entry("local", 4)).unwrap();
    changed(&mut reader, &store, "a");
    let update = reader.refresh().unwrap().unwrap();
    assert_eq!(update.traces[0].trace.trace_id, "a");
    assert_eq!(update.traces[0].trace.entry_count, 2);
    assert_eq!(update.traces[0].entries[1].reason, "entry 4 café");
    assert_eq!(
        update.retained.iter().find(|(id, _)| id == "a").unwrap().1,
        1
    );
    assert_eq!(
        update.retained.iter().find(|(id, _)| id == "b").unwrap().1,
        1
    );
}

#[test]
fn a_previously_unrelated_history_can_gain_local_entries() {
    let (_dir, store, mut reader) = setup();
    store.append("mixed", &entry("other", 1)).unwrap();
    assert!(reader.refresh().unwrap().is_none());
    store.append("mixed", &entry("local", 2)).unwrap();
    changed(&mut reader, &store, "mixed");
    let update = reader.refresh().unwrap().unwrap();
    assert_eq!(update.traces.len(), 1);
    assert_eq!(update.traces[0].entries.len(), 1);
}

#[test]
fn complete_final_records_and_blank_lines_never_duplicate_on_newline_append() {
    let (_dir, store, mut reader) = setup();
    fs::create_dir(store.trace_directory("a")).unwrap();
    fs::write(path(&store, "a"), format!("\n  \n{}", entry("local", 1))).unwrap();
    assert_eq!(
        reader.refresh().unwrap().unwrap().traces[0].entries.len(),
        1
    );
    let mut file = fs::OpenOptions::new()
        .append(true)
        .open(path(&store, "a"))
        .unwrap();
    writeln!(file).unwrap();
    changed(&mut reader, &store, "a");
    assert!(reader.refresh().unwrap().is_none());
    store.append("a", &entry("local", 2)).unwrap();
    changed(&mut reader, &store, "a");
    assert_eq!(
        reader.refresh().unwrap().unwrap().traces[0].entries.len(),
        2
    );
}

#[test]
fn partial_json_and_utf8_retry_without_another_notification() {
    let (_dir, store, mut reader) = setup();
    store.append("a", &entry("local", 1)).unwrap();
    reader.refresh().unwrap().unwrap();
    let next = format!("{}\n", entry("local", 2)).into_bytes();
    let split = next.iter().position(|byte| *byte == 0xc3).unwrap() + 1;
    let mut file = fs::OpenOptions::new()
        .append(true)
        .open(path(&store, "a"))
        .unwrap();
    file.write_all(&next[..split]).unwrap();
    changed(&mut reader, &store, "a");
    assert!(reader.refresh().unwrap().is_none());
    assert!(reader.needs_retry());
    file.write_all(&next[split..]).unwrap();
    let update = reader.refresh().unwrap().unwrap();
    assert_eq!(update.traces[0].entries.len(), 2);
    assert!(!reader.needs_retry());
    file.write_all(b"{\"v\":").unwrap();
    changed(&mut reader, &store, "a");
    assert!(reader.refresh().unwrap().is_none());
    assert!(reader.needs_retry());
}

#[test]
fn replacement_truncation_and_same_length_rewrite_invalidate_changed_sources() {
    let (_dir, store, mut reader) = setup();
    store.append("a", &entry("local", 1)).unwrap();
    store.append("a", &entry("local", 2)).unwrap();
    reader.refresh().unwrap().unwrap();
    let replacement = store.root().join("replacement");
    fs::write(&replacement, format!("{}\n", entry("local", 3))).unwrap();
    fs::rename(&replacement, path(&store, "a")).unwrap();
    changed(&mut reader, &store, "a");
    let update = reader.refresh().unwrap().unwrap();
    assert_eq!(update.traces[0].entries.len(), 1);
    assert_eq!(update.retained[0].1, 0);
    fs::write(path(&store, "a"), format!("{}\n", entry("local", 4))).unwrap();
    changed(&mut reader, &store, "a");
    let update = reader.refresh().unwrap().unwrap();
    assert_eq!(update.traces[0].entries[0].reason, "entry 4 café");
    assert_eq!(update.retained[0].1, 0);
    fs::write(path(&store, "a"), b"").unwrap();
    changed(&mut reader, &store, "a");
    assert!(reader.refresh().unwrap().unwrap().traces.is_empty());
}

#[test]
fn malformed_complete_record_retains_snapshot_and_pending_work_until_repaired() {
    let (_dir, store, mut reader) = setup();
    store.append("a", &entry("local", 1)).unwrap();
    reader.refresh().unwrap().unwrap();
    fs::write(path(&store, "a"), b"not json\n").unwrap();
    changed(&mut reader, &store, "a");
    assert!(reader.refresh().unwrap_err().contains("entry 1"));
    assert!(reader.needs_retry());
    fs::write(path(&store, "a"), format!("{}\n", entry("local", 2))).unwrap();
    assert_eq!(
        reader.refresh().unwrap().unwrap().traces[0].entries[0].reason,
        "entry 2 café"
    );
}

#[test]
fn reconciliation_finds_new_changed_and_deleted_histories_without_reparsing_unchanged_ones() {
    let (_dir, store, mut reader) = setup();
    store.append("a", &entry("local", 1)).unwrap();
    store.append("b", &entry("other", 2)).unwrap();
    reader.refresh().unwrap().unwrap();
    store.append("new", &entry("local", 3)).unwrap();
    fs::remove_dir_all(store.trace_directory("a")).unwrap();
    reader.notify(&[], true);
    let update = reader.refresh().unwrap().unwrap();
    assert_eq!(update.traces.len(), 1);
    assert_eq!(update.traces[0].trace.trace_id, "new");
    reader.notify(&[store.root().to_path_buf()], false);
    assert!(reader.refresh().unwrap().is_none());
    assert!(!reader.needs_retry());
}

#[test]
fn lock_events_are_ignored_and_pending_overflow_reconciles() {
    let (_dir, store, mut reader) = setup();
    reader.refresh().unwrap();
    assert!(!reader.notify(&[store.trace_directory("a").join("lock")], false));
    store.append("a", &entry("local", 1)).unwrap();
    let paths: Vec<_> = (0..=MAX_PENDING)
        .map(|number| path(&store, &format!("trace-{number}")))
        .collect();
    assert!(reader.notify(&paths, false));
    let update = reader.refresh().unwrap().unwrap();
    assert_eq!(update.traces[0].trace.trace_id, "a");
}

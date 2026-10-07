use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use deltoids::LineKind;
use deltoids_cli::{HistoryEntry, TraceStore};
use serde_json::Value;
use tempfile::{TempDir, tempdir};

const REQUEST: &str = r#"{"tool":"bash","command":"cargo fmt","origin":{"agent":"pi","sessionId":"s1","toolCallId":"call-1"}}"#;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/kao")
        .join(name)
}

struct Store {
    data_home: TempDir,
}

impl Store {
    fn new() -> Self {
        Self {
            data_home: tempdir().unwrap(),
        }
    }

    fn record(&self, archive: &Path, trace_id: Option<&str>) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_deltoids"));
        command
            .arg("record")
            .args(trace_id)
            .arg("--capture")
            .arg(archive)
            .env("XDG_DATA_HOME", self.data_home.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(REQUEST.as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    }

    fn record_ok(&self, archive: &Path, trace_id: Option<&str>) -> Value {
        let output = self.record(archive, trace_id);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }

    fn traces(&self) -> TraceStore {
        TraceStore::with_root(self.data_home.path().join("edit/traces"))
    }

    fn entries(&self, trace_id: &str) -> Vec<HistoryEntry> {
        self.traces().read(trace_id).unwrap()
    }
}

fn file<'a>(entry: &'a HistoryEntry, name: &str) -> &'a deltoids_cli::FileChange {
    entry
        .files
        .iter()
        .find(|file| file.path.ends_with(&format!("/{name}")))
        .unwrap_or_else(|| panic!("{name} missing from {:?}", entry.paths()))
}

fn changed_lines(change: &deltoids_cli::FileChange, kind: LineKind) -> Vec<&str> {
    change
        .hunks
        .iter()
        .flat_map(|hunk| &hunk.lines)
        .filter(|line| line.kind == kind)
        .map(|line| line.content.as_str())
        .collect()
}

fn archive_member(archive: &Path, name: &str) -> Vec<u8> {
    let mut tar = tar::Archive::new(fs::File::open(archive).unwrap());
    let mut member = tar
        .entries()
        .unwrap()
        .map(Result::unwrap)
        .find(|member| member.path().unwrap().to_str() == Some(name))
        .unwrap();
    let mut bytes = Vec::new();
    member.read_to_end(&mut bytes).unwrap();
    bytes
}

#[test]
fn records_every_changed_file_of_a_capture_as_one_entry() {
    let store = Store::new();
    let response = store.record_ok(&fixture("changes.tar"), None);
    let trace_id = response["traceId"].as_str().unwrap();

    assert_eq!(response["recorded"], true);
    assert_eq!(response["paths"].as_array().unwrap().len(), 9);
    let entries = store.entries(trace_id);
    assert_eq!(entries.len(), 1);
    let entry = &entries[0];
    assert_eq!(entry.tool, "bash");
    assert_eq!(entry.command.as_deref(), Some("cargo fmt"));
    assert_eq!(entry.reason, "cargo fmt");
    assert!(!entry.ok);
    assert_eq!(entry.error.as_deref(), Some("Command exited with code 7"));
    assert_eq!(entry.outcome.as_ref().unwrap().exit_code, Some(7));
    let origin = entry.origin.as_ref().unwrap();
    assert_eq!(
        (origin.agent.as_str(), origin.session_id.as_str()),
        ("pi", "s1")
    );
    assert_eq!(origin.tool_call_id.as_deref(), Some("call-1"));
    assert_eq!(entry.files.len(), 9);
}

#[test]
fn rebuilds_text_changes_from_the_archive_alone() {
    let store = Store::new();
    let response = store.record_ok(&fixture("changes.tar"), None);
    let entry = &store.entries(response["traceId"].as_str().unwrap())[0];

    // The before side is the uncommitted working copy, not HEAD.
    let app = file(entry, "src/app.rs");
    assert_eq!(changed_lines(app, LineKind::Removed), ["    let x = 2;"]);
    assert_eq!(
        changed_lines(app, LineKind::Added),
        ["    let x = 3;", "    let y = 4;"]
    );
    let created = file(entry, "created.txt");
    assert_eq!(
        changed_lines(created, LineKind::Added),
        ["created", "no newline"]
    );
    assert_eq!(
        changed_lines(file(entry, "deleted.txt"), LineKind::Removed),
        ["gone"]
    );
    assert_eq!(
        changed_lines(file(entry, "we*ird[1].txt"), LineKind::Added),
        ["x"]
    );
    assert!(!file(entry, "crlf.txt").hunks.is_empty());
}

#[test]
fn describes_changes_a_line_diff_cannot_show() {
    let store = Store::new();
    let response = store.record_ok(&fixture("changes.tar"), None);
    let entry = &store.entries(response["traceId"].as_str().unwrap())[0];

    let notice = |name| file(entry, name).notice();
    assert_eq!(notice("binary.dat").as_deref(), Some("Binary file changed"));
    assert_eq!(notice("link").as_deref(), Some("Symlink changed"));
    assert_eq!(
        notice("script.sh").as_deref(),
        Some("Mode changed from 100644 to 100755")
    );
    assert_eq!(notice("empty.txt").as_deref(), Some("Empty file created"));
}

#[test]
fn keeps_the_raw_patch_beside_the_trace() {
    let store = Store::new();
    let response = store.record_ok(&fixture("changes.tar"), None);
    let trace_id = response["traceId"].as_str().unwrap();
    let entry = &store.entries(trace_id)[0];

    let stored = store
        .data_home
        .path()
        .join("edit/traces")
        .join(trace_id)
        .join(entry.patch.as_deref().unwrap());
    assert_eq!(
        fs::read(stored).unwrap(),
        archive_member(&fixture("changes.tar"), "changes.patch")
    );
}

#[test]
fn recording_the_same_capture_twice_adds_one_entry() {
    let store = Store::new();
    let first = store.record_ok(&fixture("changes.tar"), None);
    let trace_id = first["traceId"].as_str().unwrap();

    let second = store.record_ok(&fixture("changes.tar"), Some(trace_id));

    assert_eq!(second["recorded"], true);
    assert_eq!(second["paths"], first["paths"]);
    assert_eq!(store.entries(trace_id).len(), 1);
}

#[test]
fn a_capture_without_changes_records_nothing() {
    let store = Store::new();
    let response = store.record_ok(&fixture("nochange.tar"), None);

    assert_eq!(response["recorded"], false);
    assert!(response.get("traceId").is_none());
    assert!(store.traces().list_all().unwrap().is_empty());
}

#[test]
fn an_incomplete_capture_is_recorded_with_its_error() {
    let store = Store::new();
    let response = store.record_ok(&fixture("incomplete.tar"), None);
    let entry = &store.entries(response["traceId"].as_str().unwrap())[0];

    assert!(!entry.ok);
    assert!(
        entry
            .error
            .as_deref()
            .unwrap()
            .starts_with("Capture incomplete: submodules and embedded repositories")
    );
    assert!(entry.files.is_empty());
}

fn rejects(store: &Store, archive: Vec<u8>, message: &str) {
    let dir = tempdir().unwrap();
    let path = dir.path().join("capture.tar");
    fs::write(&path, archive).unwrap();

    let output = store.record(&path, None);

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains(message), "{stderr}");
    assert!(store.traces().list_all().unwrap().is_empty());
}

#[test]
fn a_truncated_archive_is_rejected() {
    let archive = fs::read(fixture("changes.tar")).unwrap();
    let truncated = archive[..archive.len() / 2].to_vec();

    rejects(&Store::new(), truncated, "Invalid capture archive");
}

#[test]
fn a_blob_that_does_not_match_its_hash_is_rejected() {
    let mut archive = fs::read(fixture("changes.tar")).unwrap();
    let blob = b"created\nno newline";
    let at = archive
        .windows(blob.len())
        .position(|window| window == blob)
        .unwrap();
    archive[at] = b'C';

    rejects(&Store::new(), archive, "does not match its hash");
}

//! `record`: import a Kao capture archive as one trace entry.
//!
//! Kao runs a command and writes a tar archive describing what it changed:
//! `changes.patch`, the after contents of text files under `blobs/`, and a
//! final `result.json` manifest. This module validates that archive,
//! rebuilds every text file's before and after contents from the archive
//! alone (never from the live working tree, which may have moved on), and
//! appends one entry listing every changed file.
//!
//! Before contents come from reversing the patch with `git apply` against
//! the after blobs, then checking each result against the manifest's
//! SHA-256. Kao already requires Git, so this reuses Git's exact patch
//! semantics (CRLF, missing final newlines, quoted paths) instead of
//! reimplementing them.

use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::{Component, Path};
use std::process::Command;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::trace_store::{
    CommandOutcome, ENTRY_VERSION, FileChange, HistoryEntry, Origin, TraceStore,
};

const FORMAT_VERSION: u32 = 1;
const MANIFEST: &str = "result.json";
const PATCH: &str = "changes.patch";
const SYMLINK_MODE: &str = "120000";
const EXECUTABLE_MODE: &str = "100755";

/// Caller metadata for a capture: Kao's archive knows what changed, the
/// caller knows who ran it and why.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordRequest {
    pub tool: String,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub origin: Option<Origin>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RecordResponse {
    pub ok: bool,
    /// Whether the capture produced an entry. A complete capture with no
    /// changed files records nothing.
    pub recorded: bool,
    #[serde(rename = "traceId", skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
    #[serde(rename = "operationId")]
    pub operation_id: String,
    pub paths: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct Manifest {
    format_version: u32,
    operation_id: String,
    repository_root: String,
    command: ManifestCommand,
    capture: ManifestCapture,
    files: Vec<ManifestFile>,
}

#[derive(Debug, Deserialize)]
struct ManifestCommand {
    exit_code: Option<i32>,
    signal: Option<i32>,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ManifestCapture {
    complete: bool,
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ManifestFile {
    path: String,
    before_mode: Option<String>,
    after_mode: Option<String>,
    before_sha256: Option<String>,
    after_sha256: Option<String>,
    after_blob: Option<String>,
}

impl ManifestFile {
    fn is_symlink(&self) -> bool {
        [&self.before_mode, &self.after_mode]
            .iter()
            .any(|mode| mode.as_deref() == Some(SYMLINK_MODE))
    }

    /// Whether reversing the patch rebuilds this file's before contents:
    /// the after side is known (an after blob) or absent (a deletion).
    fn rebuildable(&self) -> bool {
        !self.is_symlink() && (self.after_blob.is_some() || self.after_mode.is_none())
    }
}

/// A validated capture archive.
struct Capture {
    manifest: Manifest,
    patch: Vec<u8>,
    blobs: HashMap<String, Vec<u8>>,
}

/// Import `archive` into the trace `trace_id` (or a new trace).
pub fn record_capture(
    store: &TraceStore,
    trace_id: Option<&str>,
    archive: &[u8],
    request: RecordRequest,
) -> Result<RecordResponse, String> {
    let capture = Capture::read(archive)?;
    let operation_id = capture.manifest.operation_id.clone();
    if capture.manifest.capture.complete && capture.manifest.files.is_empty() {
        return Ok(RecordResponse {
            ok: true,
            recorded: false,
            trace_id: trace_id.map(str::to_string),
            operation_id,
            paths: Vec::new(),
        });
    }

    let trace = store.resolve(trace_id)?;
    if trace.reused
        && let Some(existing) = store
            .read(&trace.trace_id)?
            .into_iter()
            .find(|entry| entry.operation_id.as_deref() == Some(operation_id.as_str()))
    {
        return Ok(recorded(&trace.trace_id, &operation_id, existing.paths()));
    }

    let entry = capture.entry(&trace.trace_id, request)?;
    let paths = entry.paths();
    if let Some(patch) = &entry.patch {
        let path = store.trace_directory(&trace.trace_id).join(patch);
        fs::create_dir_all(path.parent().expect("patch path has a parent"))
            .and_then(|()| fs::write(&path, &capture.patch))
            .map_err(|err| format!("Failed to store {}: {err}", path.display()))?;
    }
    store.append(&trace.trace_id, &entry)?;
    Ok(recorded(&trace.trace_id, &operation_id, paths))
}

fn recorded(trace_id: &str, operation_id: &str, paths: Vec<String>) -> RecordResponse {
    RecordResponse {
        ok: true,
        recorded: true,
        trace_id: Some(trace_id.to_string()),
        operation_id: operation_id.to_string(),
        paths,
    }
}

impl Capture {
    /// Read and validate an archive: the manifest is the final member, its
    /// format is supported, its paths stay inside the repository, and every
    /// blob it names is present and matches its hash.
    fn read(archive: &[u8]) -> Result<Self, String> {
        let mut members = Vec::new();
        let mut tar = tar::Archive::new(archive);
        for member in tar
            .entries()
            .map_err(|err| format!("Invalid capture archive: {err}"))?
        {
            let mut member = member.map_err(|err| format!("Invalid capture archive: {err}"))?;
            let name = member
                .path()
                .map_err(|err| format!("Invalid capture archive: {err}"))?
                .to_string_lossy()
                .into_owned();
            let mut bytes = Vec::new();
            member
                .read_to_end(&mut bytes)
                .map_err(|err| format!("Invalid capture archive: {err}"))?;
            members.push((name, bytes));
        }
        let Some((last, manifest)) = members.pop() else {
            return Err("Invalid capture archive: it is empty".to_string());
        };
        if last != MANIFEST {
            return Err("Invalid capture archive: it does not end with result.json".to_string());
        }
        let manifest: Manifest = serde_json::from_slice(&manifest)
            .map_err(|err| format!("Invalid capture manifest: {err}"))?;
        let mut members: HashMap<_, _> = members.into_iter().collect();
        let patch = members
            .remove(PATCH)
            .ok_or("Invalid capture archive: changes.patch is missing")?;
        let capture = Self {
            manifest,
            patch,
            blobs: members,
        };
        capture.validate()?;
        Ok(capture)
    }

    fn validate(&self) -> Result<(), String> {
        let manifest = &self.manifest;
        if manifest.format_version != FORMAT_VERSION {
            return Err(format!(
                "Unsupported capture format version {}",
                manifest.format_version
            ));
        }
        crate::trace_store::validate_trace_id(&manifest.operation_id)
            .map_err(|_| format!("Invalid operation id: {}", manifest.operation_id))?;
        manifest
            .files
            .iter()
            .try_for_each(|file| self.validate_file(file))
    }

    /// A file's path stays inside the repository and its after blob, if
    /// any, is present and matches its hash.
    fn validate_file(&self, file: &ManifestFile) -> Result<(), String> {
        let inside = Path::new(&file.path)
            .components()
            .all(|part| matches!(part, Component::Normal(_)));
        if !inside {
            return Err(format!("Invalid capture path: {}", file.path));
        }
        let Some(blob) = &file.after_blob else {
            return Ok(());
        };
        let bytes = self
            .blobs
            .get(blob)
            .ok_or_else(|| format!("Capture is missing {blob}"))?;
        if Some(sha256(bytes)) != file.after_sha256 {
            return Err(format!("Capture blob does not match its hash: {blob}"));
        }
        Ok(())
    }

    /// The entry describing this capture.
    fn entry(&self, trace_id: &str, request: RecordRequest) -> Result<HistoryEntry, String> {
        let manifest = &self.manifest;
        let before = self.rebuild_before()?;
        let files = manifest
            .files
            .iter()
            .map(|file| self.file_change(file, &before))
            .collect();
        let error = describe_failure(manifest);
        Ok(HistoryEntry {
            v: ENTRY_VERSION,
            tool: request.tool.clone(),
            trace_id: trace_id.to_string(),
            timestamp: crate::current_timestamp(),
            cwd: crate::current_working_directory()?,
            reason: request
                .reason
                .or_else(|| request.command.clone())
                .unwrap_or(request.tool),
            ok: error.is_none(),
            error,
            files,
            command: request.command,
            outcome: Some(CommandOutcome {
                exit_code: manifest.command.exit_code,
                signal: manifest.command.signal,
            }),
            operation_id: Some(manifest.operation_id.clone()),
            origin: request.origin,
            patch: (!self.patch.is_empty())
                .then(|| format!("patches/{}.patch", manifest.operation_id)),
        })
    }

    /// One file of the entry: hashes and modes always; hunks when both sides
    /// are known text.
    fn file_change(&self, file: &ManifestFile, before: &HashMap<String, Vec<u8>>) -> FileChange {
        let mut change = FileChange {
            path: Path::new(&self.manifest.repository_root)
                .join(&file.path)
                .to_string_lossy()
                .into_owned(),
            before_sha256: file.before_sha256.clone(),
            after_sha256: file.after_sha256.clone(),
            before_mode: file.before_mode.clone(),
            after_mode: file.after_mode.clone(),
            ..FileChange::default()
        };
        let before = match file.before_sha256 {
            None => Some(""),
            Some(_) => before.get(&file.path).and_then(|bytes| text(bytes)),
        };
        let after = match &file.after_blob {
            Some(blob) => self.blobs.get(blob).and_then(|bytes| text(bytes)),
            None if file.after_mode.is_none() => Some(""),
            None => None,
        };
        if let (Some(before), Some(after)) = (before, after) {
            let computed = deltoids::Diff::compute(before, after, &change.path);
            change.hunks = computed.hunks().to_vec();
            change.diff = Some(computed.text().to_string());
            change.language = computed.language();
            change.highlight = computed.highlight().map(str::to_string);
        }
        change
    }

    /// Before contents of every rebuildable file, keyed by manifest path.
    /// Files whose rebuild fails or does not match its hash are left out;
    /// the entry still lists them, without hunks.
    fn rebuild_before(&self) -> Result<HashMap<String, Vec<u8>>, String> {
        let files: Vec<&ManifestFile> = self
            .manifest
            .files
            .iter()
            .filter(|file| file.rebuildable() && file.before_sha256.is_some())
            .collect();
        if files.is_empty() {
            return Ok(HashMap::new());
        }
        let work = Workspace::new(&self.patch)?;
        for file in self.manifest.files.iter().filter(|file| file.rebuildable()) {
            if let Some(blob) = &file.after_blob {
                work.write(&file.path, &self.blobs[blob], file.after_mode.as_deref())?;
            }
        }
        let paths: Vec<&str> = files.iter().map(|file| file.path.as_str()).collect();
        if !work.reverse(&paths) {
            for path in &paths {
                work.reverse(&[path]);
            }
        }
        Ok(files
            .into_iter()
            .filter_map(|file| {
                let bytes = fs::read(work.root.join(&file.path)).ok()?;
                (Some(sha256(&bytes)) == file.before_sha256).then(|| (file.path.clone(), bytes))
            })
            .collect())
    }
}

/// A scratch Git repository holding after contents and the patch.
struct Workspace {
    _dir: tempfile::TempDir,
    root: std::path::PathBuf,
    patch: std::path::PathBuf,
}

impl Workspace {
    fn new(patch: &[u8]) -> Result<Self, String> {
        let dir =
            tempfile::tempdir().map_err(|err| format!("Failed to create a work dir: {err}"))?;
        let root = dir.path().join("tree");
        fs::create_dir(&root).map_err(|err| format!("Failed to create a work dir: {err}"))?;
        let patch_path = dir.path().join(PATCH);
        fs::write(&patch_path, patch).map_err(|err| format!("Failed to write the patch: {err}"))?;
        let status = Command::new("git")
            .args(["init", "-q"])
            .current_dir(&root)
            .status()
            .map_err(|err| format!("Failed to run git: {err}"))?;
        if !status.success() {
            return Err("git init failed while rebuilding a capture".to_string());
        }
        Ok(Self {
            _dir: dir,
            root,
            patch: patch_path,
        })
    }

    fn write(&self, path: &str, bytes: &[u8], mode: Option<&str>) -> Result<(), String> {
        let target = self.root.join(path);
        let parent = target.parent().expect("a capture path has a parent");
        fs::create_dir_all(parent)
            .and_then(|()| fs::write(&target, bytes))
            .map_err(|err| format!("Failed to write {path}: {err}"))?;
        #[cfg(unix)]
        if mode == Some(EXECUTABLE_MODE) {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&target, fs::Permissions::from_mode(0o755))
                .map_err(|err| format!("Failed to chmod {path}: {err}"))?;
        }
        Ok(())
    }

    /// Reverse the patch for `paths` only. Returns whether Git applied it.
    fn reverse(&self, paths: &[&str]) -> bool {
        let mut command = Command::new("git");
        command
            .args(["apply", "-R", "--binary"])
            .current_dir(&self.root);
        for path in paths {
            command.arg(format!("--include={}", escape_pattern(path)));
        }
        command
            .arg(&self.patch)
            .output()
            .is_ok_and(|output| output.status.success())
    }
}

/// `git apply --include` takes a pattern; match `path` literally.
fn escape_pattern(path: &str) -> String {
    let mut escaped = String::with_capacity(path.len());
    for ch in path.chars() {
        if matches!(ch, '*' | '?' | '[' | '\\') {
            escaped.push('\\');
        }
        escaped.push(ch);
    }
    escaped
}

/// Text a line diff can show: UTF-8 without NUL bytes.
fn text(bytes: &[u8]) -> Option<&str> {
    std::str::from_utf8(bytes)
        .ok()
        .filter(|text| !text.contains('\0'))
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// Why a captured invocation failed, if it did: an incomplete capture, a
/// command that never started, a signal, or a nonzero exit.
fn describe_failure(manifest: &Manifest) -> Option<String> {
    if !manifest.capture.complete {
        return Some(format!(
            "Capture incomplete: {}",
            manifest.capture.error.as_deref().unwrap_or("unknown error")
        ));
    }
    let command = &manifest.command;
    if let Some(error) = &command.error {
        return Some(format!("Command could not start: {error}"));
    }
    match (command.exit_code, command.signal) {
        (_, Some(signal)) => Some(format!("Command ended by signal {signal}")),
        (Some(0), None) => None,
        (Some(code), None) => Some(format!("Command exited with code {code}")),
        (None, None) => Some("Command ended without an exit code".to_string()),
    }
}

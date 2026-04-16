use std::env;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::{SecondsFormat, Utc};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use similar::TextDiff;
use ulid::Ulid;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EditRequest {
    pub summary: String,
    pub path: String,
    pub edits: Vec<TextEdit>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TextEdit {
    pub summary: String,
    #[serde(rename = "oldText")]
    pub old_text: String,
    #[serde(rename = "newText")]
    pub new_text: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WriteRequest {
    pub summary: String,
    pub path: String,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SuccessResponse {
    pub ok: bool,
    pub path: String,
    #[serde(rename = "traceId")]
    pub trace_id: String,
    pub message: String,
    pub diff: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ErrorResponse {
    pub ok: bool,
    pub error: String,
    #[serde(rename = "traceId", skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolError {
    pub error: String,
    pub trace_id: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MatchedEdit {
    index: usize,
    start: usize,
    end: usize,
    new_text: String,
}

#[derive(Debug, Serialize)]
struct EditHistoryEntry {
    v: u8,
    tool: &'static str,
    #[serde(rename = "traceId")]
    trace_id: String,
    timestamp: String,
    cwd: String,
    path: String,
    summary: String,
    ok: bool,
    edits: Vec<TextEdit>,
    diff: String,
}

#[derive(Debug, Serialize)]
struct EditFailureHistoryEntry {
    v: u8,
    tool: &'static str,
    #[serde(rename = "traceId")]
    trace_id: String,
    timestamp: String,
    cwd: String,
    path: String,
    summary: String,
    ok: bool,
    edits: Vec<TextEdit>,
    error: String,
}

#[derive(Debug, Serialize)]
struct WriteHistoryEntry {
    v: u8,
    tool: &'static str,
    #[serde(rename = "traceId")]
    trace_id: String,
    timestamp: String,
    cwd: String,
    path: String,
    summary: String,
    ok: bool,
    content: String,
    diff: String,
}

#[derive(Debug, Serialize)]
struct WriteFailureHistoryEntry {
    v: u8,
    tool: &'static str,
    #[serde(rename = "traceId")]
    trace_id: String,
    timestamp: String,
    cwd: String,
    path: String,
    summary: String,
    ok: bool,
    content: String,
    error: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct HistoryEntry {
    pub v: u8,
    pub tool: String,
    #[serde(rename = "traceId")]
    pub trace_id: String,
    pub timestamp: String,
    pub cwd: String,
    pub path: String,
    pub summary: String,
    pub ok: bool,
    #[serde(default)]
    pub edits: Vec<TextEdit>,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub diff: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
}

pub fn execute_request(request: EditRequest) -> Result<SuccessResponse, ToolError> {
    execute_request_with_trace(request, None)
}

pub fn execute_request_with_trace(
    request: EditRequest,
    trace_id: Option<&str>,
) -> Result<SuccessResponse, ToolError> {
    let reused_trace = trace_id.is_some();
    let trace_id = trace_id
        .map(str::to_owned)
        .unwrap_or_else(|| Ulid::new().to_string());

    match try_execute_edit(&request, &trace_id, reused_trace) {
        Ok(response) => Ok(response),
        Err(error) => Err(log_edit_failure(request, trace_id, reused_trace, error)),
    }
}

pub fn execute_write_request(request: WriteRequest) -> Result<SuccessResponse, ToolError> {
    execute_write_request_with_trace(request, None)
}

pub fn execute_write_request_with_trace(
    request: WriteRequest,
    trace_id: Option<&str>,
) -> Result<SuccessResponse, ToolError> {
    let reused_trace = trace_id.is_some();
    let trace_id = trace_id
        .map(str::to_owned)
        .unwrap_or_else(|| Ulid::new().to_string());

    match try_execute_write(&request, &trace_id, reused_trace) {
        Ok(response) => Ok(response),
        Err(error) => Err(log_write_failure(request, trace_id, reused_trace, error)),
    }
}

fn try_execute_edit(
    request: &EditRequest,
    trace_id: &str,
    reused_trace: bool,
) -> Result<SuccessResponse, String> {
    validate_request(request)?;

    let path = Path::new(&request.path);
    validate_target_path(path, &request.path)?;

    let original = fs::read_to_string(path)
        .map_err(|err| format!("Failed to read {}: {}", request.path, err))?;
    let updated = apply_edits(&original, &request.edits, &request.path)?;
    let diff = render_diff(&original, &updated);

    fs::write(path, &updated)
        .map_err(|err| format!("Failed to write {}: {}", request.path, err))?;

    append_trace_entry(
        trace_id,
        &EditHistoryEntry {
            v: 1,
            tool: "edit",
            trace_id: trace_id.to_string(),
            timestamp: current_timestamp(),
            cwd: current_working_directory()?,
            path: request.path.clone(),
            summary: request.summary.clone(),
            ok: true,
            edits: request.edits.clone(),
            diff: diff.clone(),
        },
    )?;

    Ok(SuccessResponse {
        ok: true,
        path: request.path.clone(),
        trace_id: trace_id.to_string(),
        message: success_message(trace_id, reused_trace),
        diff,
    })
}

fn try_execute_write(
    request: &WriteRequest,
    trace_id: &str,
    reused_trace: bool,
) -> Result<SuccessResponse, String> {
    validate_write_request(request)?;

    let path = Path::new(&request.path);
    validate_write_target_path(path, &request.path)?;

    let original = if path.exists() {
        fs::read_to_string(path)
            .map_err(|err| format!("Failed to read {}: {}", request.path, err))?
    } else {
        String::new()
    };
    let diff = render_diff(&original, &request.content);

    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).map_err(|err| {
                format!(
                    "Failed to create parent directories for {}: {}",
                    request.path, err
                )
            })?;
        }
    }

    fs::write(path, &request.content)
        .map_err(|err| format!("Failed to write {}: {}", request.path, err))?;

    append_trace_entry(
        trace_id,
        &WriteHistoryEntry {
            v: 1,
            tool: "write",
            trace_id: trace_id.to_string(),
            timestamp: current_timestamp(),
            cwd: current_working_directory()?,
            path: request.path.clone(),
            summary: request.summary.clone(),
            ok: true,
            content: request.content.clone(),
            diff: diff.clone(),
        },
    )?;

    Ok(SuccessResponse {
        ok: true,
        path: request.path.clone(),
        trace_id: trace_id.to_string(),
        message: success_message(trace_id, reused_trace),
        diff,
    })
}

fn success_message(trace_id: &str, reused_trace: bool) -> String {
    if reused_trace {
        format!("Appended to trace {trace_id}.")
    } else {
        format!(
            "Started trace {trace_id}. Reuse this trace id for later edits in the same session."
        )
    }
}

fn log_edit_failure(
    request: EditRequest,
    trace_id: String,
    reused_trace: bool,
    error: String,
) -> ToolError {
    let logging_error = append_trace_entry(
        &trace_id,
        &EditFailureHistoryEntry {
            v: 1,
            tool: "edit",
            trace_id: trace_id.clone(),
            timestamp: current_timestamp(),
            cwd: current_working_directory().unwrap_or_else(|_| String::new()),
            path: request.path,
            summary: request.summary,
            ok: false,
            edits: request.edits,
            error: error.clone(),
        },
    )
    .err();

    tool_error(trace_id, reused_trace, error, logging_error)
}

fn log_write_failure(
    request: WriteRequest,
    trace_id: String,
    reused_trace: bool,
    error: String,
) -> ToolError {
    let logging_error = append_trace_entry(
        &trace_id,
        &WriteFailureHistoryEntry {
            v: 1,
            tool: "write",
            trace_id: trace_id.clone(),
            timestamp: current_timestamp(),
            cwd: current_working_directory().unwrap_or_else(|_| String::new()),
            path: request.path,
            summary: request.summary,
            ok: false,
            content: request.content,
            error: error.clone(),
        },
    )
    .err();

    tool_error(trace_id, reused_trace, error, logging_error)
}

fn tool_error(
    trace_id: String,
    reused_trace: bool,
    error: String,
    logging_error: Option<String>,
) -> ToolError {
    ToolError {
        error: match logging_error {
            Some(logging_error) => {
                format!("{error} Failed to record trace {trace_id}: {logging_error}")
            }
            None => error,
        },
        trace_id: trace_id.clone(),
        message: success_message(&trace_id, reused_trace),
    }
}

fn current_timestamp() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn current_working_directory() -> Result<String, String> {
    env::current_dir()
        .map(|path| path.to_string_lossy().into_owned())
        .map_err(|err| format!("Failed to read current directory: {err}"))
}

fn append_trace_entry<T: Serialize>(trace_id: &str, entry: &T) -> Result<(), String> {
    let trace_dir = trace_directory(trace_id)?;
    fs::create_dir_all(&trace_dir).map_err(|err| {
        format!(
            "Failed to create trace directory {}: {}",
            trace_dir.display(),
            err
        )
    })?;

    let lock_path = trace_dir.join(".lock");
    let lock_file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(&lock_path)
        .map_err(|err| format!("Failed to open trace lock {}: {}", lock_path.display(), err))?;
    lock_file
        .lock_exclusive()
        .map_err(|err| format!("Failed to lock trace {}: {}", trace_id, err))?;

    let result = (|| {
        let entries_path = trace_dir.join("entries.jsonl");
        let mut entries_file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&entries_path)
            .map_err(|err| {
                format!(
                    "Failed to open trace entries {}: {}",
                    entries_path.display(),
                    err
                )
            })?;
        serde_json::to_writer(&mut entries_file, entry)
            .map_err(|err| format!("Failed to serialize trace entry: {err}"))?;
        writeln!(&mut entries_file).map_err(|err| {
            format!(
                "Failed to append trace entry {}: {}",
                entries_path.display(),
                err
            )
        })
    })();

    let unlock_result = lock_file.unlock();
    result?;
    unlock_result.map_err(|err| format!("Failed to unlock trace {}: {}", trace_id, err))?;
    Ok(())
}

pub fn read_history_entries(trace_id: &str) -> Result<Vec<HistoryEntry>, String> {
    let entries_path = trace_directory(trace_id)?.join("entries.jsonl");
    if !entries_path.exists() {
        return Err(format!("Trace not found: {trace_id}"));
    }

    let contents = fs::read_to_string(&entries_path)
        .map_err(|err| format!("Failed to read {}: {}", entries_path.display(), err))?;
    let mut entries = Vec::new();
    for (index, line) in contents.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }

        let entry = serde_json::from_str(line).map_err(|err| {
            format!(
                "Failed to parse history entry {} in {}: {}",
                index + 1,
                entries_path.display(),
                err
            )
        })?;
        entries.push(entry);
    }

    Ok(entries)
}

fn trace_directory(trace_id: &str) -> Result<PathBuf, String> {
    Ok(trace_root_directory()?.join(trace_id))
}

fn trace_root_directory() -> Result<PathBuf, String> {
    Ok(data_home_directory()?.join("edit").join("traces"))
}

fn data_home_directory() -> Result<PathBuf, String> {
    if let Some(path) = env::var_os("XDG_DATA_HOME") {
        return Ok(PathBuf::from(path));
    }

    if let Some(home) = env::var_os("HOME") {
        return Ok(PathBuf::from(home).join(".local").join("share"));
    }

    Err("Could not determine data home directory".to_string())
}

fn validate_request(request: &EditRequest) -> Result<(), String> {
    if request.summary.trim().is_empty() {
        return Err("summary must not be empty".to_string());
    }

    if request.edits.is_empty() {
        return Err("edits must contain at least one replacement".to_string());
    }

    for (index, edit) in request.edits.iter().enumerate() {
        if edit.summary.trim().is_empty() {
            return Err(format!("edits[{index}].summary must not be empty"));
        }

        if edit.old_text.is_empty() {
            return Err(format!("edits[{index}].oldText must not be empty"));
        }
    }

    Ok(())
}

fn validate_write_request(request: &WriteRequest) -> Result<(), String> {
    if request.summary.trim().is_empty() {
        return Err("summary must not be empty".to_string());
    }

    Ok(())
}

pub fn validate_target_path(path: &Path, display_path: &str) -> Result<(), String> {
    if !path.exists() {
        return Err(format!("Path does not exist: {display_path}"));
    }

    if !path.is_file() {
        return Err(format!("Path is not a file: {display_path}"));
    }

    Ok(())
}

pub fn validate_write_target_path(path: &Path, display_path: &str) -> Result<(), String> {
    if path.exists() && !path.is_file() {
        return Err(format!("Path is not a file: {display_path}"));
    }

    Ok(())
}

pub fn render_diff(original: &str, updated: &str) -> String {
    let text_diff = TextDiff::from_lines(original, updated);
    let mut diff = text_diff.unified_diff();
    diff.context_radius(3).header("original", "modified");
    diff.to_string()
}

pub fn apply_edits(original: &str, edits: &[TextEdit], path: &str) -> Result<String, String> {
    let mut matches = Vec::with_capacity(edits.len());

    for (index, edit) in edits.iter().enumerate() {
        let occurrences = original.match_indices(&edit.old_text).collect::<Vec<_>>();
        match occurrences.len() {
            0 => {
                return Err(format!(
                    "Could not find edits[{index}] in {path}. The oldText must match exactly."
                ));
            }
            1 => {
                let (start, matched) = occurrences[0];
                matches.push(MatchedEdit {
                    index,
                    start,
                    end: start + matched.len(),
                    new_text: edit.new_text.clone(),
                });
            }
            count => {
                return Err(format!(
                    "Found {count} occurrences of edits[{index}] in {path}. Each oldText must be unique."
                ));
            }
        }
    }

    matches.sort_by_key(|m| m.start);
    for pair in matches.windows(2) {
        let previous = &pair[0];
        let current = &pair[1];
        if previous.end > current.start {
            return Err(format!(
                "edits[{}] and edits[{}] overlap in {}. Merge them into one edit or target disjoint regions.",
                previous.index, current.index, path
            ));
        }
    }

    let mut result = original.to_string();
    for matched in matches.iter().rev() {
        result.replace_range(matched.start..matched.end, &matched.new_text);
    }

    if result == original {
        return Err(format!(
            "No changes made to {path}. The replacements produced identical content."
        ));
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{
        EditRequest, TextEdit, WriteRequest, apply_edits, execute_write_request, render_diff,
        validate_request,
    };

    #[test]
    fn applies_single_exact_edit() {
        let result = apply_edits(
            "Hello, world!",
            &[TextEdit {
                summary: "Replace world".to_string(),
                old_text: "world".to_string(),
                new_text: "pi".to_string(),
            }],
            "test.txt",
        )
        .unwrap();

        assert_eq!(result, "Hello, pi!");
    }

    #[test]
    fn applies_multiple_disjoint_edits_against_original_content() {
        let result = apply_edits(
            "foo\nbar\nbaz\n",
            &[
                TextEdit {
                    summary: "Expand foo".to_string(),
                    old_text: "foo\n".to_string(),
                    new_text: "foo bar\n".to_string(),
                },
                TextEdit {
                    summary: "Uppercase bar".to_string(),
                    old_text: "bar\n".to_string(),
                    new_text: "BAR\n".to_string(),
                },
            ],
            "test.txt",
        )
        .unwrap();

        assert_eq!(result, "foo bar\nBAR\nbaz\n");
    }

    #[test]
    fn rejects_missing_text() {
        let error = apply_edits(
            "hello\n",
            &[TextEdit {
                summary: "Replace missing".to_string(),
                old_text: "missing".to_string(),
                new_text: "x".to_string(),
            }],
            "test.txt",
        )
        .unwrap_err();

        assert!(error.contains("Could not find"));
    }

    #[test]
    fn rejects_duplicate_text() {
        let error = apply_edits(
            "foo foo foo",
            &[TextEdit {
                summary: "Replace foo".to_string(),
                old_text: "foo".to_string(),
                new_text: "bar".to_string(),
            }],
            "test.txt",
        )
        .unwrap_err();

        assert!(error.contains("Found 3 occurrences"));
    }

    #[test]
    fn rejects_overlapping_regions() {
        let error = apply_edits(
            "one\ntwo\nthree\n",
            &[
                TextEdit {
                    summary: "Uppercase first block".to_string(),
                    old_text: "one\ntwo\n".to_string(),
                    new_text: "ONE\nTWO\n".to_string(),
                },
                TextEdit {
                    summary: "Uppercase second block".to_string(),
                    old_text: "two\nthree\n".to_string(),
                    new_text: "TWO\nTHREE\n".to_string(),
                },
            ],
            "test.txt",
        )
        .unwrap_err();

        assert!(error.contains("overlap"));
    }

    #[test]
    fn rejects_no_op_replacement() {
        let error = apply_edits(
            "same",
            &[TextEdit {
                summary: "No-op replace".to_string(),
                old_text: "same".to_string(),
                new_text: "same".to_string(),
            }],
            "test.txt",
        )
        .unwrap_err();

        assert!(error.contains("No changes made"));
    }

    #[test]
    fn rejects_empty_edits_request() {
        let request = EditRequest {
            summary: "Test edit".to_string(),
            path: "test.txt".to_string(),
            edits: Vec::new(),
        };

        let error = validate_request(&request).unwrap_err();
        assert!(error.contains("edits must contain at least one replacement"));
    }

    #[test]
    fn rejects_empty_edit_summary() {
        let request = EditRequest {
            summary: "Test edit".to_string(),
            path: "test.txt".to_string(),
            edits: vec![TextEdit {
                summary: String::new(),
                old_text: "before".to_string(),
                new_text: "after".to_string(),
            }],
        };

        let error = validate_request(&request).unwrap_err();
        assert!(error.contains("edits[0].summary must not be empty"));
    }

    #[test]
    fn rejects_whitespace_only_edit_summary() {
        let request = EditRequest {
            summary: "Test edit".to_string(),
            path: "test.txt".to_string(),
            edits: vec![TextEdit {
                summary: " \n\t ".to_string(),
                old_text: "before".to_string(),
                new_text: "after".to_string(),
            }],
        };

        let error = validate_request(&request).unwrap_err();
        assert!(error.contains("edits[0].summary must not be empty"));
    }

    #[test]
    fn rejects_empty_old_text() {
        let request = EditRequest {
            summary: "Test edit".to_string(),
            path: "test.txt".to_string(),
            edits: vec![TextEdit {
                summary: "Replace text".to_string(),
                old_text: String::new(),
                new_text: "replacement".to_string(),
            }],
        };

        let error = validate_request(&request).unwrap_err();
        assert!(error.contains("edits[0].oldText must not be empty"));
    }

    #[test]
    fn rejects_empty_summary() {
        let request = EditRequest {
            summary: String::new(),
            path: "test.txt".to_string(),
            edits: vec![TextEdit {
                summary: "Replace before".to_string(),
                old_text: "before".to_string(),
                new_text: "after".to_string(),
            }],
        };

        let error = validate_request(&request).unwrap_err();
        assert!(error.contains("summary must not be empty"));
    }

    #[test]
    fn rejects_whitespace_only_summary() {
        let request = EditRequest {
            summary: " \n\t ".to_string(),
            path: "test.txt".to_string(),
            edits: vec![TextEdit {
                summary: "Replace before".to_string(),
                old_text: "before".to_string(),
                new_text: "after".to_string(),
            }],
        };

        let error = validate_request(&request).unwrap_err();
        assert!(error.contains("summary must not be empty"));
    }

    #[test]
    fn writes_full_content_and_returns_diff() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        fs::write(&path, "{\n  \"version\": 1\n}\n").unwrap();

        let response = execute_write_request(WriteRequest {
            summary: "Rewrite config".to_string(),
            path: path.to_string_lossy().into_owned(),
            content: "{\n  \"version\": 2\n}\n".to_string(),
        })
        .unwrap();

        assert!(response.ok);
        assert!(response.trace_id.len() >= 10);
        assert!(response.diff.contains("-  \"version\": 1"));
        assert!(response.diff.contains("+  \"version\": 2"));
        assert_eq!(
            fs::read_to_string(path).unwrap(),
            "{\n  \"version\": 2\n}\n"
        );
    }

    #[test]
    fn rejects_empty_write_summary() {
        let error = execute_write_request(WriteRequest {
            summary: String::new(),
            path: "test.txt".to_string(),
            content: "hello\n".to_string(),
        })
        .unwrap_err();

        assert!(error.error.contains("summary must not be empty"));
    }

    #[test]
    fn does_not_partially_apply_when_one_multi_edit_fails() {
        let original = "alpha\nbeta\ngamma\n";
        let error = apply_edits(
            original,
            &[
                TextEdit {
                    summary: "Uppercase alpha".to_string(),
                    old_text: "alpha\n".to_string(),
                    new_text: "ALPHA\n".to_string(),
                },
                TextEdit {
                    summary: "Try missing line".to_string(),
                    old_text: "missing\n".to_string(),
                    new_text: "MISSING\n".to_string(),
                },
            ],
            "test.txt",
        )
        .unwrap_err();

        assert!(error.contains("Could not find"));
        assert_eq!(original, "alpha\nbeta\ngamma\n");
    }

    #[test]
    fn renders_a_line_based_diff() {
        let diff = render_diff("const x = 1;\n", "const x = 2;\n");

        assert!(diff.contains("--- original"));
        assert!(diff.contains("+++ modified"));
        assert!(diff.contains("-const x = 1;"));
        assert!(diff.contains("+const x = 2;"));
    }

    #[test]
    fn renders_multiple_changes_in_one_diff() {
        let diff = render_diff("alpha\nbeta\ngamma\ndelta\n", "ALPHA\nbeta\nGAMMA\ndelta\n");

        assert!(diff.contains("-alpha"));
        assert!(diff.contains("+ALPHA"));
        assert!(diff.contains("-gamma"));
        assert!(diff.contains("+GAMMA"));
    }
}

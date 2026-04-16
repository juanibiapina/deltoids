use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};
use similar::TextDiff;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EditRequest {
    pub summary: String,
    pub path: String,
    pub edits: Vec<TextEdit>,
}

#[derive(Debug, Clone, Deserialize)]
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
    #[serde(rename = "replacedBlocks")]
    pub replaced_blocks: usize,
    pub diff: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WriteSuccessResponse {
    pub ok: bool,
    pub path: String,
    pub diff: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ErrorResponse {
    pub ok: bool,
    pub error: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MatchedEdit {
    index: usize,
    start: usize,
    end: usize,
    new_text: String,
}

pub fn execute_request(request: EditRequest) -> Result<SuccessResponse, String> {
    validate_request(&request)?;

    let path = Path::new(&request.path);
    validate_target_path(path, &request.path)?;

    let original = fs::read_to_string(path)
        .map_err(|err| format!("Failed to read {}: {}", request.path, err))?;
    let updated = apply_edits(&original, &request.edits, &request.path)?;
    let diff = render_diff(&original, &updated);

    fs::write(path, &updated)
        .map_err(|err| format!("Failed to write {}: {}", request.path, err))?;

    Ok(SuccessResponse {
        ok: true,
        path: request.path,
        replaced_blocks: request.edits.len(),
        diff,
    })
}

pub fn execute_write_request(request: WriteRequest) -> Result<WriteSuccessResponse, String> {
    validate_write_request(&request)?;

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

    Ok(WriteSuccessResponse {
        ok: true,
        path: request.path,
        diff,
    })
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

        assert!(error.contains("summary must not be empty"));
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

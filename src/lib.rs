use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

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
    #[serde(rename = "oldText")]
    pub old_text: String,
    #[serde(rename = "newText")]
    pub new_text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SuccessResponse {
    pub ok: bool,
    pub path: String,
    #[serde(rename = "replacedBlocks")]
    pub replaced_blocks: usize,
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
    let original = fs::read_to_string(path)
        .map_err(|err| format!("Failed to read {}: {}", request.path, err))?;
    let updated = apply_edits(&original, &request.edits, &request.path)?;

    fs::write(path, updated).map_err(|err| format!("Failed to write {}: {}", request.path, err))?;

    Ok(SuccessResponse {
        ok: true,
        path: request.path,
        replaced_blocks: request.edits.len(),
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
        if edit.old_text.is_empty() {
            return Err(format!("edits[{index}].oldText must not be empty"));
        }
    }

    Ok(())
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
    use super::{TextEdit, apply_edits};

    #[test]
    fn applies_single_exact_edit() {
        let result = apply_edits(
            "Hello, world!",
            &[TextEdit {
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
                    old_text: "foo\n".to_string(),
                    new_text: "foo bar\n".to_string(),
                },
                TextEdit {
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
                    old_text: "one\ntwo\n".to_string(),
                    new_text: "ONE\nTWO\n".to_string(),
                },
                TextEdit {
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
        let request = super::EditRequest {
            summary: "Test edit".to_string(),
            path: "test.txt".to_string(),
            edits: Vec::new(),
        };

        let error = super::validate_request(&request).unwrap_err();
        assert!(error.contains("edits must contain at least one replacement"));
    }

    #[test]
    fn rejects_empty_old_text() {
        let request = super::EditRequest {
            summary: "Test edit".to_string(),
            path: "test.txt".to_string(),
            edits: vec![TextEdit {
                old_text: String::new(),
                new_text: "replacement".to_string(),
            }],
        };

        let error = super::validate_request(&request).unwrap_err();
        assert!(error.contains("edits[0].oldText must not be empty"));
    }

    #[test]
    fn rejects_empty_summary() {
        let request = super::EditRequest {
            summary: String::new(),
            path: "test.txt".to_string(),
            edits: vec![TextEdit {
                old_text: "before".to_string(),
                new_text: "after".to_string(),
            }],
        };

        let error = super::validate_request(&request).unwrap_err();
        assert!(error.contains("summary must not be empty"));
    }

    #[test]
    fn rejects_whitespace_only_summary() {
        let request = super::EditRequest {
            summary: " \n\t ".to_string(),
            path: "test.txt".to_string(),
            edits: vec![TextEdit {
                old_text: "before".to_string(),
                new_text: "after".to_string(),
            }],
        };

        let error = super::validate_request(&request).unwrap_err();
        assert!(error.contains("summary must not be empty"));
    }

    #[test]
    fn does_not_partially_apply_when_one_multi_edit_fails() {
        let original = "alpha\nbeta\ngamma\n";
        let error = apply_edits(
            original,
            &[
                TextEdit {
                    old_text: "alpha\n".to_string(),
                    new_text: "ALPHA\n".to_string(),
                },
                TextEdit {
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
}

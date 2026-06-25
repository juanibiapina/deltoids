//! `hashread` tool: read a file and render each line as `LINEhh|content`
//! so the agent can anchor later `hashedit` operations.

use std::fs;
use std::path::Path;

use crate::{HashReadRequest, hashline, validate_target_path};

/// Read `path` and return the formatted hashline body (each line as
/// `LINEhh|content`, joined with `\n`). `offset` is the 1-indexed first
/// line to return (default `1`). `limit` is the maximum number of lines
/// (default: all remaining).
pub fn execute_hash_read(request: &HashReadRequest) -> Result<String, String> {
    let path = Path::new(&request.path);
    validate_target_path(path, &request.path)?;
    let content = fs::read_to_string(path)
        .map_err(|err| format!("Failed to read {}: {}", request.path, err))?;
    Ok(render_hash_read(&content, request.offset, request.limit))
}

fn render_hash_read(content: &str, offset: Option<usize>, limit: Option<usize>) -> String {
    // Strip a single trailing newline so we don't render a phantom empty
    // anchored line at the end of every file.
    let body = content.strip_suffix('\n').unwrap_or(content);
    let start = offset.unwrap_or(1).max(1);
    let lines: Vec<&str> = body.split('\n').collect();
    if start > lines.len() {
        return String::new();
    }
    let end = match limit {
        Some(n) => (start + n).min(lines.len() + 1),
        None => lines.len() + 1,
    };
    let mut out = String::new();
    for (i, line) in lines[start - 1..end - 1].iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        out.push_str(&hashline::format_hash_line(start + i, line));
    }
    out
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn render_hash_read_prefixes_every_line_with_anchor() {
        let body = render_hash_read("alpha\nbeta\ngamma\n", None, None);
        let lines: Vec<&str> = body.split('\n').collect();
        assert_eq!(lines.len(), 3);
        assert!(lines[0].starts_with("1"));
        assert!(lines[0].ends_with("|alpha"));
        assert!(lines[2].ends_with("|gamma"));
    }

    #[test]
    fn render_hash_read_respects_offset_and_limit() {
        let body = render_hash_read("a\nb\nc\nd\ne\n", Some(2), Some(2));
        let lines: Vec<&str> = body.split('\n').collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].starts_with("2"));
        assert!(lines[0].ends_with("|b"));
        assert!(lines[1].starts_with("3"));
        assert!(lines[1].ends_with("|c"));
    }

    #[test]
    fn execute_hash_read_returns_anchored_file_body() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sample.txt");
        fs::write(&path, "alpha\nbeta\n").unwrap();

        let body = execute_hash_read(&HashReadRequest {
            path: path.to_string_lossy().into_owned(),
            offset: None,
            limit: None,
        })
        .unwrap();

        assert!(body.contains("|alpha"));
        assert!(body.contains("|beta"));
    }

    #[test]
    fn execute_hash_read_errors_when_path_missing() {
        let err = execute_hash_read(&HashReadRequest {
            path: "/no/such/file/xyz".to_string(),
            offset: None,
            limit: None,
        })
        .unwrap_err();
        assert!(err.contains("Path does not exist"));
    }
}

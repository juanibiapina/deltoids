//! Wire types: the JSON request shapes the `edit`/`write`/`hashedit`/
//! `hashread` tools accept, plus the success/error responses and the
//! internal [`ToolError`] carried back to the CLI layer.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize)]
pub struct EditRequest {
    pub reason: String,
    pub path: String,
    #[serde(rename = "oldText")]
    pub old_text: String,
    #[serde(rename = "newText")]
    pub new_text: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TextEdit {
    #[serde(alias = "summary")] // back-compat with v1/v2 trace entries that used `summary`
    pub reason: String,
    #[serde(rename = "oldText")]
    pub old_text: String,
    #[serde(rename = "newText")]
    pub new_text: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WriteRequest {
    pub reason: String,
    pub path: String,
    pub content: String,
}

/// A single hashline edit operation as it arrives over JSON. Intent is
/// carried by the request's top-level `reason`; ops hold only mechanics.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum HashEditOp {
    /// Replace one anchored line, or the inclusive range `pos..=end` if
    /// `end` is provided, with `lines`.
    Replace {
        pos: String,
        #[serde(default)]
        end: Option<String>,
        #[serde(default)]
        lines: Vec<String>,
    },
    /// Insert `lines` before the anchored line. `pos` may be `"BOF"`.
    InsertBefore { pos: String, lines: Vec<String> },
    /// Insert `lines` after the anchored line. `pos` may be `"EOF"`.
    InsertAfter { pos: String, lines: Vec<String> },
    /// Delete one anchored line, or the inclusive range `pos..=end` if
    /// `end` is provided.
    Delete {
        pos: String,
        #[serde(default)]
        end: Option<String>,
    },
}

#[derive(Debug, Clone, Deserialize)]
pub struct HashEditRequest {
    pub reason: String,
    pub path: String,
    pub edits: Vec<HashEditOp>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct HashReadRequest {
    pub path: String,
    #[serde(default)]
    pub offset: Option<usize>,
    #[serde(default)]
    pub limit: Option<usize>,
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

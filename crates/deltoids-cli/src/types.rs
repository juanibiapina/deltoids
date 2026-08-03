//! Wire types: the JSON request shapes the `edit` and `write` tools
//! accept, plus the success/error responses and the internal [`ToolError`]
//! carried back to the CLI layer.

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

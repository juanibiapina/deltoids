//! Pure request routing for `deltoids serve`.
//!
//! [`handle`] maps a method + request target to an [`HttpResponse`] against a
//! [`TraceStore`], with no socket involved, so every route is unit-testable.
//! The socket loop in [`super`] is a thin shell that calls this.

use std::path::Path;

use serde::Serialize;

use deltoids::render_html::render_entry_html;

use crate::{HistoryEntry, TraceStore, project_id};

use super::assets;

/// A fully-formed HTTP response: status, content type, and body bytes.
pub struct HttpResponse {
    pub status: u16,
    pub content_type: &'static str,
    pub body: Vec<u8>,
}

impl HttpResponse {
    fn json(value: &impl Serialize) -> Self {
        match serde_json::to_vec(value) {
            Ok(body) => Self {
                status: 200,
                content_type: "application/json",
                body,
            },
            Err(err) => Self::text(500, format!("serialize error: {err}")),
        }
    }

    fn asset(content_type: &'static str, body: &'static str) -> Self {
        Self {
            status: 200,
            content_type,
            body: body.as_bytes().to_vec(),
        }
    }

    fn text(status: u16, message: impl Into<String>) -> Self {
        Self {
            status,
            content_type: "text/plain; charset=utf-8",
            body: message.into().into_bytes(),
        }
    }

    fn not_found() -> Self {
        Self::text(404, "not found")
    }
}

/// Route a request to a response. `target` is the raw request URL
/// (`/path?query`).
pub fn handle(store: &TraceStore, method: &str, target: &str) -> HttpResponse {
    if method != "GET" {
        return HttpResponse::text(405, "method not allowed");
    }

    let (path, query) = split_target(target);
    let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();

    match segments.as_slice() {
        [] | ["index.html"] => HttpResponse::asset("text/html; charset=utf-8", assets::INDEX_HTML),
        ["app.js"] => HttpResponse::asset("text/javascript; charset=utf-8", assets::APP_JS),
        ["style.css"] => HttpResponse::asset("text/css; charset=utf-8", assets::STYLE_CSS),
        ["api", "projects"] => projects(store),
        ["api", "projects", id, "traces"] => project_traces(store, id),
        ["api", "traces", trace_id, "entries"] => trace_entries(store, trace_id),
        ["api", "traces", trace_id, "entries", index] => trace_entry(store, trace_id, index),
        ["api", "feed"] => feed(store, query),
        _ => HttpResponse::not_found(),
    }
}

fn projects(store: &TraceStore) -> HttpResponse {
    match store.projects() {
        Ok(projects) => HttpResponse::json(&projects),
        Err(err) => HttpResponse::text(500, err),
    }
}

fn project_traces(store: &TraceStore, id: &str) -> HttpResponse {
    // Resolve the opaque project id back to a cwd, then list traces there.
    let projects = match store.projects() {
        Ok(projects) => projects,
        Err(err) => return HttpResponse::text(500, err),
    };
    let Some(project) = projects.into_iter().find(|p| p.id == id) else {
        return HttpResponse::not_found();
    };
    match store.list_all() {
        Ok(traces) => {
            let for_project: Vec<_> = traces
                .into_iter()
                .filter(|trace| trace.cwd == project.cwd)
                .collect();
            HttpResponse::json(&for_project)
        }
        Err(err) => HttpResponse::text(500, err),
    }
}

#[derive(Serialize)]
struct EntryMeta {
    index: usize,
    tool: String,
    path: String,
    reason: String,
    ok: bool,
    timestamp: String,
}

#[derive(Serialize)]
struct TraceEntries {
    trace_id: String,
    cwd: String,
    entries: Vec<EntryMeta>,
}

fn trace_entries(store: &TraceStore, trace_id: &str) -> HttpResponse {
    let entries = match store.read(trace_id) {
        Ok(entries) => entries,
        Err(err) => return HttpResponse::text(404, err),
    };
    let cwd = entries
        .last()
        .map(|entry| entry.cwd.clone())
        .unwrap_or_default();
    let metas = entries
        .iter()
        .enumerate()
        .map(|(index, entry)| EntryMeta {
            index,
            tool: entry.tool.clone(),
            path: entry.path.clone(),
            reason: entry.reason.clone(),
            ok: entry.ok,
            timestamp: entry.timestamp.clone(),
        })
        .collect();
    HttpResponse::json(&TraceEntries {
        trace_id: trace_id.to_string(),
        cwd,
        entries: metas,
    })
}

#[derive(Serialize)]
struct EntryDetail {
    index: usize,
    tool: String,
    path: String,
    reason: String,
    ok: bool,
    timestamp: String,
    error: Option<String>,
    html: String,
}

fn trace_entry(store: &TraceStore, trace_id: &str, index: &str) -> HttpResponse {
    let Ok(index) = index.parse::<usize>() else {
        return HttpResponse::not_found();
    };
    let entries = match store.read(trace_id) {
        Ok(entries) => entries,
        Err(err) => return HttpResponse::text(404, err),
    };
    let Some(entry) = entries.get(index) else {
        return HttpResponse::not_found();
    };
    let html = entry_html(entry);
    HttpResponse::json(&EntryDetail {
        index,
        tool: entry.tool.clone(),
        path: entry.path.clone(),
        reason: entry.reason.clone(),
        ok: entry.ok,
        timestamp: entry.timestamp.clone(),
        error: entry.error.clone(),
        html,
    })
}

/// Render an entry's diff body, or an explanatory placeholder for entries
/// that carry no hunks (errors, or legacy v1 traces).
fn entry_html(entry: &HistoryEntry) -> String {
    if !entry.hunks.is_empty() {
        return render_entry_html(&entry.hunks, entry.highlight.as_deref());
    }
    if !entry.ok {
        return String::new(); // the error text is delivered as a field
    }
    "<div class=\"notice\">Old trace format; diff cannot be shown.</div>".to_string()
}

#[derive(Serialize)]
struct FeedEntry {
    trace_id: String,
    index: usize,
    project_id: String,
    project_name: String,
    cwd: String,
    tool: String,
    path: String,
    reason: String,
    ok: bool,
    timestamp: String,
}

#[derive(Serialize)]
struct Feed {
    cursor: String,
    entries: Vec<FeedEntry>,
}

/// Entries newer than the `since` query timestamp, across every trace,
/// newest first. `cursor` echoes the newest timestamp so the client can
/// pass it back on the next poll.
fn feed(store: &TraceStore, query: &str) -> HttpResponse {
    let since = query_value(query, "since").unwrap_or_default();
    let traces = match store.list_all() {
        Ok(traces) => traces,
        Err(err) => return HttpResponse::text(500, err),
    };

    let mut entries = Vec::new();
    for trace in traces {
        let loaded = match store.read(&trace.trace_id) {
            Ok(loaded) => loaded,
            Err(_) => continue,
        };
        for (index, entry) in loaded.into_iter().enumerate() {
            if !since.is_empty() && entry.timestamp < since {
                continue;
            }
            entries.push(FeedEntry {
                trace_id: trace.trace_id.clone(),
                index,
                project_id: project_id(&entry.cwd),
                project_name: project_name(&entry.cwd),
                cwd: entry.cwd,
                tool: entry.tool,
                path: entry.path,
                reason: entry.reason,
                ok: entry.ok,
                timestamp: entry.timestamp,
            });
        }
    }

    entries.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
    let cursor = entries
        .first()
        .map(|entry| entry.timestamp.clone())
        .unwrap_or(since.to_string());
    HttpResponse::json(&Feed { cursor, entries })
}

fn project_name(cwd: &str) -> String {
    Path::new(cwd)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| cwd.to_string())
}

fn split_target(target: &str) -> (&str, &str) {
    match target.split_once('?') {
        Some((path, query)) => (path, query),
        None => (target, ""),
    }
}

/// Extract and percent-decode a query parameter value.
fn query_value(query: &str, key: &str) -> Option<String> {
    query.split('&').find_map(|pair| {
        let (name, value) = pair.split_once('=')?;
        if name == key {
            Some(percent_decode(value))
        } else {
            None
        }
    })
}

/// Minimal percent-decoding (also turns `+` into a space), enough for the
/// RFC3339 timestamps the client sends as the feed cursor.
fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hi = hex_val(bytes[i + 1]);
                let lo = hex_val(bytes[i + 2]);
                match (hi, lo) {
                    (Some(hi), Some(lo)) => {
                        out.push(hi << 4 | lo);
                        i += 3;
                    }
                    _ => {
                        out.push(b'%');
                        i += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            byte => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_val(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store_with_entries() -> (tempfile::TempDir, TraceStore) {
        let tmp = tempfile::tempdir().unwrap();
        let store = TraceStore::with_root(tmp.path().to_path_buf());
        append(
            &store,
            "01JAAAAAAAAAAAAAAAAAAAAAAA",
            "/proj",
            "2026-01-01T00:00:00Z",
        );
        append(
            &store,
            "01JAAAAAAAAAAAAAAAAAAAAAAA",
            "/proj",
            "2026-01-01T00:01:00Z",
        );
        append(
            &store,
            "01JBBBBBBBBBBBBBBBBBBBBBBB",
            "/other",
            "2026-01-02T00:00:00Z",
        );
        (tmp, store)
    }

    fn append(store: &TraceStore, trace_id: &str, cwd: &str, timestamp: &str) {
        store
            .append(
                trace_id,
                &serde_json::json!({
                    "v": 3,
                    "tool": "edit",
                    "traceId": trace_id,
                    "timestamp": timestamp,
                    "cwd": cwd,
                    "path": format!("{cwd}/app.rs"),
                    "reason": "change",
                    "ok": true,
                    "hunks": [{
                        "old_start": 1,
                        "new_start": 1,
                        "lines": [{"kind": "Added", "content": "let x = 1;"}],
                        "ancestors": []
                    }]
                }),
            )
            .unwrap();
    }

    fn body_str(response: &HttpResponse) -> String {
        String::from_utf8(response.body.clone()).unwrap()
    }

    #[test]
    fn index_and_assets_are_served() {
        let tmp = tempfile::tempdir().unwrap();
        let store = TraceStore::with_root(tmp.path().to_path_buf());
        assert_eq!(handle(&store, "GET", "/").status, 200);
        assert_eq!(
            handle(&store, "GET", "/app.js").content_type,
            "text/javascript; charset=utf-8"
        );
        assert_eq!(
            handle(&store, "GET", "/style.css").content_type,
            "text/css; charset=utf-8"
        );
    }

    #[test]
    fn non_get_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let store = TraceStore::with_root(tmp.path().to_path_buf());
        assert_eq!(handle(&store, "POST", "/api/projects").status, 405);
    }

    #[test]
    fn projects_endpoint_lists_all_projects() {
        let (_tmp, store) = store_with_entries();
        let response = handle(&store, "GET", "/api/projects");
        assert_eq!(response.status, 200);
        let body = body_str(&response);
        assert!(body.contains("/proj"));
        assert!(body.contains("/other"));
    }

    #[test]
    fn project_traces_endpoint_filters_by_project() {
        let (_tmp, store) = store_with_entries();
        let id = project_id("/proj");
        let response = handle(&store, "GET", &format!("/api/projects/{id}/traces"));
        assert_eq!(response.status, 200);
        let body = body_str(&response);
        assert!(body.contains("01JAAAAAAAAAAAAAAAAAAAAAAA"));
        assert!(!body.contains("01JBBBBBBBBBBBBBBBBBBBBBBB"));
    }

    #[test]
    fn unknown_project_id_is_404() {
        let (_tmp, store) = store_with_entries();
        assert_eq!(
            handle(&store, "GET", "/api/projects/deadbeef/traces").status,
            404
        );
    }

    #[test]
    fn trace_entries_endpoint_lists_metadata() {
        let (_tmp, store) = store_with_entries();
        let response = handle(
            &store,
            "GET",
            "/api/traces/01JAAAAAAAAAAAAAAAAAAAAAAA/entries",
        );
        assert_eq!(response.status, 200);
        let body = body_str(&response);
        assert!(body.contains("\"index\":0"));
        assert!(body.contains("\"index\":1"));
        assert!(body.contains("/proj"));
    }

    #[test]
    fn trace_entry_endpoint_renders_html() {
        let (_tmp, store) = store_with_entries();
        let response = handle(
            &store,
            "GET",
            "/api/traces/01JAAAAAAAAAAAAAAAAAAAAAAA/entries/0",
        );
        assert_eq!(response.status, 200);
        let body = body_str(&response);
        assert!(body.contains("row added"));
    }

    #[test]
    fn missing_entry_is_404() {
        let (_tmp, store) = store_with_entries();
        assert_eq!(
            handle(
                &store,
                "GET",
                "/api/traces/01JAAAAAAAAAAAAAAAAAAAAAAA/entries/99"
            )
            .status,
            404
        );
    }

    #[test]
    fn feed_returns_entries_newer_than_cursor() {
        let (_tmp, store) = store_with_entries();
        // since is url-encoded (: -> %3A).
        let response = handle(&store, "GET", "/api/feed?since=2026-01-01T00%3A01%3A00Z");
        assert_eq!(response.status, 200);
        let body = body_str(&response);
        // The 00:00:00 entry is older and excluded; the 00:01 and other's
        // 00:02 remain, newest first.
        assert!(body.contains("/other"));
        assert!(body.contains("2026-01-02T00:00:00Z"));
    }

    #[test]
    fn percent_decode_handles_timestamps() {
        assert_eq!(
            percent_decode("2026-01-01T00%3A01%3A00Z"),
            "2026-01-01T00:01:00Z"
        );
        assert_eq!(percent_decode("a+b"), "a b");
    }
}

//! The `Lsp` interface against scripted fake servers.

use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

use deltoids::Language;
use lsp_server::{Message, Notification, Request, RequestId};
use notify::EventKind;
use notify::event::{AccessKind, AccessMode, CreateKind, ModifyKind, RemoveKind};
use serde_json::{Value, json};

use super::fake::{self, Reply, answering};
use super::*;

const ROOT: &str = "/work/shop";

const MAIN: &str = "mod cart;\n\nfn main() {\n    let total = cart::total();\n}\n";

fn utf8() -> Value {
    json!({ "positionEncoding": "utf-8", "callHierarchyProvider": true })
}

fn uri(path: &str) -> String {
    format!("file://{ROOT}/{path}")
}

fn location(path: &str, line: u32) -> Value {
    json!({
        "uri": uri(path),
        "range": { "start": { "line": line, "character": 0 }, "end": { "line": line, "character": 1 } }
    })
}

/// `name` on 1-based `line` of `text`, in a Rust file at `path`.
fn symbol<'a>(
    language: Language,
    path: &'a str,
    text: &'a str,
    line: usize,
    name: &str,
) -> Symbol<'a> {
    let start = text.lines().nth(line - 1).unwrap().find(name).unwrap();
    Symbol {
        language,
        path,
        text,
        line,
        columns: start..start + name.len(),
    }
}

fn total(text: &str) -> Symbol<'_> {
    symbol(Language::Rust, "src/main.rs", text, 4, "total()")
}

fn wait(lsp: &Lsp, ticket: &Ticket) -> Result<Vec<Found>, String> {
    places(wait_reply(lsp, ticket))
}

fn wait_reply(lsp: &Lsp, ticket: &Ticket) -> Result<super::Reply, String> {
    for _ in 0..400 {
        if let Some(answer) = lsp.answer(ticket) {
            return answer;
        }
        thread::sleep(Duration::from_millis(5));
    }
    panic!("no answer");
}

fn places(answer: Result<super::Reply, String>) -> Result<Vec<Found>, String> {
    match answer? {
        super::Reply::Places(found) => Ok(found),
        super::Reply::Info(text) => panic!("expected places, got info: {text}"),
    }
}

fn found(path: &str, line: usize, caller: Option<&str>) -> Found {
    Found {
        path: path.to_string(),
        line,
        caller: caller.map(str::to_string),
    }
}

fn info(contents: Value) -> Result<super::Reply, String> {
    let (lsp, _) = fake::lsp(
        Path::new(ROOT),
        utf8(),
        answering("textDocument/hover", json!({ "contents": contents })),
    );
    let ticket = lsp.ask(Query::Hover, total(MAIN)).unwrap();
    wait_reply(&lsp, &ticket)
}

#[test]
fn hover_contents_of_every_shape_come_back_as_markdown() {
    let text = |text: &str| Ok(super::Reply::Info(text.to_string()));
    assert_eq!(
        info(json!({ "kind": "markdown", "value": "```rust\nfn total() -> u32\n```" })),
        text("```rust\nfn total() -> u32\n```")
    );
    assert_eq!(
        info(json!({ "language": "rust", "value": "fn total() -> u32" })),
        text("```rust\nfn total() -> u32\n```")
    );
    assert_eq!(
        info(json!(["Adds up *items*.", { "language": "rust", "value": "fn total()" }, ""])),
        text("Adds up *items*.\n\n```rust\nfn total()\n```")
    );
    assert_eq!(
        info(json!({ "kind": "plaintext", "value": "total() -> u32\nsums *all*\n\nmore" })),
        text("total\\(\\) \\-\\> u32\\\nsums \\*all\\*\n\nmore")
    );
}

#[test]
fn a_hover_with_nothing_to_show_says_so() {
    assert_eq!(info(json!("")), Err("No info found".to_string()));
    let (lsp, _) = fake::lsp(
        Path::new(ROOT),
        utf8(),
        answering("textDocument/hover", Value::Null),
    );
    let ticket = lsp.ask(Query::Hover, total(MAIN)).unwrap();
    assert_eq!(wait_reply(&lsp, &ticket), Err("No info found".to_string()));
}

#[test]
fn a_hover_asks_at_the_symbol_and_the_client_says_it_reads_markdown() {
    let (lsp, record) = fake::lsp(
        Path::new(ROOT),
        utf8(),
        answering("textDocument/hover", json!({ "contents": "total" })),
    );
    let ticket = lsp.ask(Query::Hover, total(MAIN)).unwrap();
    wait_reply(&lsp, &ticket).unwrap();
    let asked = &record.params("textDocument/hover")[0];
    assert_eq!(asked["textDocument"]["uri"], json!(uri("src/main.rs")));
    assert_eq!(asked["position"], json!({ "line": 3, "character": 22 }));
    let initialize = &record.params("initialize")[0];
    assert_eq!(
        initialize["capabilities"]["textDocument"]["hover"]["contentFormat"],
        json!(["markdown", "plaintext"])
    );
}

#[test]
fn a_definition_location_comes_back_workdir_relative() {
    let (lsp, _) = fake::lsp(
        Path::new(ROOT),
        utf8(),
        answering("textDocument/definition", location("src/cart.rs", 9)),
    );
    let ticket = lsp.ask(Query::Definition, total(MAIN)).unwrap();
    assert_eq!(
        wait(&lsp, &ticket),
        Ok(vec![found("src/cart.rs", 10, None)])
    );
}

#[test]
fn definition_links_and_arrays_become_locations_without_repeats() {
    let links = json!([{
        "targetUri": uri("src/cart.rs"),
        "targetRange": { "start": { "line": 7, "character": 0 }, "end": { "line": 12, "character": 1 } },
        "targetSelectionRange": { "start": { "line": 8, "character": 4 }, "end": { "line": 8, "character": 9 } }
    }]);
    let (lsp, _) = fake::lsp(
        Path::new(ROOT),
        utf8(),
        answering("textDocument/definition", links),
    );
    let ticket = lsp.ask(Query::Definition, total(MAIN)).unwrap();
    assert_eq!(wait(&lsp, &ticket), Ok(vec![found("src/cart.rs", 9, None)]));

    let mut elsewhere = location("src/b.rs", 2);
    elsewhere["uri"] = json!("file:///elsewhere/b.rs");
    let array = json!([location("src/a.rs", 0), location("src/a.rs", 0), elsewhere]);
    let (lsp, _) = fake::lsp(
        Path::new(ROOT),
        utf8(),
        answering("textDocument/definition", array),
    );
    let ticket = lsp.ask(Query::Definition, total(MAIN)).unwrap();
    assert_eq!(
        wait(&lsp, &ticket),
        Ok(vec![
            found("src/a.rs", 1, None),
            found("/elsewhere/b.rs", 3, None)
        ])
    );
}

#[test]
fn callers_come_from_the_call_hierarchy_with_their_function_names() {
    let item = json!({
        "name": "total", "kind": 12, "uri": uri("src/cart.rs"),
        "range": { "start": { "line": 9, "character": 0 }, "end": { "line": 11, "character": 1 } },
        "selectionRange": { "start": { "line": 9, "character": 7 }, "end": { "line": 9, "character": 12 } },
        "data": { "opaque": 7 }
    });
    let caller = |name: &str, path: &str, lines: &[u32]| {
        let mut from = item.clone();
        from["name"] = json!(name);
        from["uri"] = json!(uri(path));
        let ranges: Vec<Value> = lines
            .iter()
            .map(|line| json!({ "start": { "line": line, "character": 4 }, "end": { "line": line, "character": 9 } }))
            .collect();
        json!({ "from": from, "fromRanges": ranges })
    };
    let calls = json!([
        caller("main", "src/main.rs", &[3]),
        caller("summary", "src/report.rs", &[3, 5])
    ]);
    let prepared = json!([item]);
    let script: fake::Script = Arc::new(move |message| match message {
        Message::Request(r) if r.method == "textDocument/prepareCallHierarchy" => {
            Reply::Result(prepared.clone())
        }
        Message::Request(r) if r.method == "callHierarchy/incomingCalls" => {
            Reply::Result(calls.clone())
        }
        _ => Reply::Default,
    });
    let (lsp, record) = fake::lsp(Path::new(ROOT), utf8(), script);
    let ticket = lsp.ask(Query::Callers, total(MAIN)).unwrap();
    assert_eq!(
        wait(&lsp, &ticket),
        Ok(vec![
            found("src/main.rs", 4, Some("main")),
            found("src/report.rs", 4, Some("summary")),
            found("src/report.rs", 6, Some("summary")),
        ])
    );
    let incoming = record.params("callHierarchy/incomingCalls");
    assert_eq!(incoming[0]["item"]["data"], json!({ "opaque": 7 }));
}

#[test]
fn callers_fall_back_to_references_without_a_call_hierarchy() {
    let (lsp, record) = fake::lsp(
        Path::new(ROOT),
        json!({ "positionEncoding": "utf-8" }),
        answering(
            "textDocument/references",
            json!([location("src/report.rs", 3)]),
        ),
    );
    let ticket = lsp.ask(Query::Callers, total(MAIN)).unwrap();
    assert_eq!(
        wait(&lsp, &ticket),
        Ok(vec![found("src/report.rs", 4, None)])
    );
    let params = record.params("textDocument/references");
    assert_eq!(params[0]["context"]["includeDeclaration"], json!(false));
}

fn with_declarations() -> Value {
    json!({
        "positionEncoding": "utf-8",
        "callHierarchyProvider": true,
        "declarationProvider": true
    })
}

fn range(line: u32) -> Value {
    json!({ "start": { "line": line, "character": 7 }, "end": { "line": line, "character": 12 } })
}

/// A call hierarchy item for `total` declared on 0-based `line` of
/// `src/cart.rs`.
fn total_item(name: &str, line: u32) -> Value {
    json!({
        "name": name, "kind": 6, "uri": uri("src/cart.rs"),
        "range": range(line), "selectionRange": range(line)
    })
}

const IMPL_LINE: u32 = 9;
const TRAIT_LINE: u32 = 2;
const OTHER_IMPL_LINE: u32 = 15;

/// A server where `total` at the cursor is an impl method on
/// `IMPL_LINE` of its trait's method on `TRAIT_LINE`, and the
/// declaration of the cursor is on `declared`. The trait method has four
/// callers in `src/report.rs`, whose calls go to: the impl (`direct`,
/// line 10), the trait (`shell`, line 20), another impl (`mock`, line
/// 30), and the trait (`dynamic`, line 40). The impl alone has
/// `direct`. `references` at the cursor gives `direct`; at the trait's
/// method it also gives `shell` and the `fn` names of the impl and of
/// another impl. With `silent`, no `definition` answers.
fn trait_server(declared: u32, silent: bool) -> fake::Script {
    let call = |name: &str, line: u32| {
        json!({
            "from": { "name": name, "kind": 12, "uri": uri("src/report.rs"),
                      "range": range(line), "selectionRange": range(line) },
            "fromRanges": [{ "start": { "line": line, "character": 4 },
                             "end": { "line": line, "character": 9 } }]
        })
    };
    let trait_calls = json!([
        call("direct", 10),
        call("shell", 20),
        call("mock", 30),
        call("dynamic", 40)
    ]);
    let impl_calls = json!([call("direct", 10)]);
    let at = |line: u32, character: u32| {
        json!({
            "uri": uri(if line == IMPL_LINE || line == OTHER_IMPL_LINE { "src/cart.rs" } else { "src/report.rs" }),
            "range": { "start": { "line": line, "character": character },
                       "end": { "line": line, "character": character + 5 } }
        })
    };
    let cursor_references = json!([at(10, 4)]);
    let trait_references = json!([
        at(10, 4),
        at(20, 4),
        at(IMPL_LINE, 7),
        at(OTHER_IMPL_LINE, 7)
    ]);
    Arc::new(move |message| {
        let Message::Request(r) = message else {
            return Reply::Default;
        };
        let line = r.params["position"]["line"].as_u64();
        match r.method.as_str() {
            "textDocument/declaration" => {
                Reply::Result(json!([{ "uri": uri("src/cart.rs"), "range": range(declared) }]))
            }
            "textDocument/prepareCallHierarchy" if line == Some(TRAIT_LINE.into()) => {
                Reply::Result(json!([total_item("trait", TRAIT_LINE)]))
            }
            "textDocument/prepareCallHierarchy" => {
                Reply::Result(json!([total_item("impl", IMPL_LINE)]))
            }
            "callHierarchy/incomingCalls" if r.params["item"]["name"] == "trait" => {
                Reply::Result(trait_calls.clone())
            }
            "callHierarchy/incomingCalls" => Reply::Result(impl_calls.clone()),
            "textDocument/references" if line == Some(TRAIT_LINE.into()) => {
                Reply::Result(trait_references.clone())
            }
            "textDocument/references" => Reply::Result(cursor_references.clone()),
            "textDocument/definition" if silent => Reply::Silent,
            "textDocument/definition" => {
                let target = match line {
                    Some(10) => IMPL_LINE,
                    Some(30) => OTHER_IMPL_LINE,
                    Some(line) if line == u64::from(OTHER_IMPL_LINE) => OTHER_IMPL_LINE,
                    _ => TRAIT_LINE,
                };
                Reply::Result(json!({ "uri": uri("src/cart.rs"), "range": range(target) }))
            }
            _ => Reply::Default,
        }
    })
}

fn definition_lines(record: &fake::Record) -> Vec<u64> {
    let mut lines: Vec<u64> = record
        .params("textDocument/definition")
        .iter()
        .filter_map(|params| params["position"]["line"].as_u64())
        .collect();
    lines.sort_unstable();
    lines
}

#[test]
fn callers_of_an_impl_method_include_calls_through_its_trait() {
    let (lsp, record) = fake::lsp(
        Path::new(ROOT),
        with_declarations(),
        trait_server(TRAIT_LINE, false),
    );
    let ticket = lsp.ask(Query::Callers, total(MAIN)).unwrap();
    assert_eq!(
        wait(&lsp, &ticket),
        Ok(vec![
            found("src/report.rs", 11, Some("direct")),
            found("src/report.rs", 21, Some("shell")),
            found("src/report.rs", 41, Some("dynamic")),
        ])
    );
    assert_eq!(definition_lines(&record), vec![15, 20, 30, 40]);
}

#[test]
fn a_declaration_where_the_function_is_keeps_its_own_callers() {
    let (lsp, record) = fake::lsp(
        Path::new(ROOT),
        with_declarations(),
        trait_server(IMPL_LINE, false),
    );
    let ticket = lsp.ask(Query::Callers, total(MAIN)).unwrap();
    assert_eq!(
        wait(&lsp, &ticket),
        Ok(vec![found("src/report.rs", 11, Some("direct"))])
    );
    assert!(definition_lines(&record).is_empty());
}

#[test]
fn without_declarations_callers_skip_the_trait() {
    let (lsp, record) = fake::lsp(Path::new(ROOT), utf8(), trait_server(TRAIT_LINE, false));
    let ticket = lsp.ask(Query::Callers, total(MAIN)).unwrap();
    assert_eq!(
        wait(&lsp, &ticket),
        Ok(vec![found("src/report.rs", 11, Some("direct"))])
    );
    assert!(record.params("textDocument/declaration").is_empty());
}

#[test]
fn a_call_whose_definition_fails_stays_a_caller() {
    let script = trait_server(TRAIT_LINE, false);
    let failing: fake::Script = Arc::new(move |message| match message {
        Message::Request(r)
            if r.method == "textDocument/definition" && r.params["position"]["line"] == 30 =>
        {
            Reply::Error("content modified".to_string())
        }
        _ => script(message),
    });
    let (lsp, _) = fake::lsp(Path::new(ROOT), with_declarations(), failing);
    let ticket = lsp.ask(Query::Callers, total(MAIN)).unwrap();
    assert_eq!(
        wait(&lsp, &ticket),
        Ok(vec![
            found("src/report.rs", 11, Some("direct")),
            found("src/report.rs", 21, Some("shell")),
            found("src/report.rs", 31, Some("mock")),
            found("src/report.rs", 41, Some("dynamic")),
        ])
    );
}

/// A server with no call hierarchy item at the cursor, as for a
/// constant, answering `references` with `locations`.
fn referenced_only(locations: Value) -> fake::Script {
    Arc::new(move |message| match message {
        Message::Request(r) if r.method == "textDocument/prepareCallHierarchy" => {
            Reply::Result(json!([]))
        }
        Message::Request(r) if r.method == "textDocument/references" => {
            Reply::Result(locations.clone())
        }
        _ => Reply::Default,
    })
}

#[test]
fn a_symbol_that_is_not_a_function_lists_its_uses() {
    let (lsp, _) = fake::lsp(
        Path::new(ROOT),
        with_declarations(),
        referenced_only(json!([
            location("src/report.rs", 7),
            location("src/main.rs", 2)
        ])),
    );
    let ticket = lsp.ask(Query::Callers, total(MAIN)).unwrap();
    assert_eq!(
        wait(&lsp, &ticket),
        Ok(vec![
            found("src/main.rs", 3, None),
            found("src/report.rs", 8, None),
        ])
    );
}

#[test]
fn a_function_used_outside_any_function_lists_that_use() {
    let item = total_item("total", IMPL_LINE);
    let call = json!([{
        "from": { "name": "main", "kind": 12, "uri": uri("src/main.rs"),
                  "range": range(2), "selectionRange": range(2) },
        "fromRanges": [{ "start": { "line": 3, "character": 4 }, "end": { "line": 3, "character": 9 } }]
    }]);
    let use_at = |path: &str, line: u32, character: u32| {
        json!({ "uri": uri(path), "range": {
            "start": { "line": line, "character": character },
            "end": { "line": line, "character": character + 5 } } })
    };
    let references = json!([
        use_at("src/main.rs", 3, 4),
        use_at("src/report.rs", 0, 20),
        use_at("src/other.rs", 5, 7)
    ]);
    let script: fake::Script = Arc::new(move |message| {
        let Message::Request(r) = message else {
            return Reply::Default;
        };
        let line = r.params["position"]["line"].as_u64();
        match r.method.as_str() {
            "textDocument/prepareCallHierarchy" => Reply::Result(json!([item.clone()])),
            "callHierarchy/incomingCalls" => Reply::Result(call.clone()),
            "textDocument/references" => Reply::Result(references.clone()),
            "textDocument/definition" if line == Some(0) => {
                Reply::Result(json!({ "uri": uri("src/cart.rs"), "range": range(IMPL_LINE) }))
            }
            "textDocument/definition" => {
                Reply::Result(json!({ "uri": uri("src/other.rs"), "range": range(5) }))
            }
            _ => Reply::Default,
        }
    });
    let (lsp, record) = fake::lsp(Path::new(ROOT), utf8(), script);
    let ticket = lsp.ask(Query::Callers, total(MAIN)).unwrap();
    assert_eq!(
        wait(&lsp, &ticket),
        Ok(vec![
            found("src/main.rs", 4, Some("main")),
            found("src/report.rs", 1, None),
        ])
    );
    assert_eq!(definition_lines(&record), vec![0, 5]);
}

#[test]
fn callers_survive_a_failing_references_request() {
    let script = trait_server(IMPL_LINE, false);
    let failing: fake::Script = Arc::new(move |message| match message {
        Message::Request(r) if r.method == "textDocument/references" => {
            Reply::Error("references broke".to_string())
        }
        _ => script(message),
    });
    let (lsp, _) = fake::lsp(Path::new(ROOT), with_declarations(), failing);
    let ticket = lsp.ask(Query::Callers, total(MAIN)).unwrap();
    assert_eq!(
        wait(&lsp, &ticket),
        Ok(vec![found("src/report.rs", 11, Some("direct"))])
    );
}

#[test]
fn callers_fail_when_every_request_fails() {
    let script: fake::Script = Arc::new(|message| match message {
        Message::Request(r) if r.method != "initialize" && r.method != "shutdown" => {
            Reply::Error("broken".to_string())
        }
        _ => Reply::Default,
    });
    let (lsp, _) = fake::lsp(Path::new(ROOT), with_declarations(), script);
    let ticket = lsp.ask(Query::Callers, total(MAIN)).unwrap();
    let answer = wait(&lsp, &ticket);
    assert!(
        answer.as_ref().is_err_and(|error| error.contains("broken")),
        "{answer:?}"
    );
}

#[test]
fn a_cancelled_trait_lookup_never_answers() {
    let (lsp, record) = fake::lsp(
        Path::new(ROOT),
        with_declarations(),
        trait_server(TRAIT_LINE, true),
    );
    let ticket = lsp.ask(Query::Callers, total(MAIN)).unwrap();
    assert!(eventually(|| {
        lsp.state(Language::Rust);
        definition_lines(&record).len() == 4
    }));
    let probe = Ticket(ticket.0);
    lsp.cancel(ticket);
    assert!(eventually(|| record.params("$/cancelRequest").len() == 4));
    thread::sleep(Duration::from_millis(30));
    assert_eq!(lsp.answer(&probe), None);
}

#[test]
fn a_utf16_server_gets_columns_in_utf16_units() {
    let (lsp, record) = fake::lsp(
        Path::new(ROOT),
        json!({}),
        answering("textDocument/definition", location("src/cart.rs", 0)),
    );
    let text = "fn main() {\n    let é😀 = go();\n}\n";
    let ticket = lsp
        .ask(
            Query::Definition,
            symbol(Language::Rust, "src/main.rs", text, 2, "go"),
        )
        .unwrap();
    wait(&lsp, &ticket).unwrap();
    let position = &record.params("textDocument/definition")[0]["position"];
    assert_eq!(position, &json!({ "line": 1, "character": 14 }));
}

#[test]
fn documents_open_once_and_change_only_when_their_text_does() {
    let (lsp, record) = fake::lsp(
        Path::new(ROOT),
        utf8(),
        answering("textDocument/definition", location("src/cart.rs", 0)),
    );
    for _ in 0..2 {
        let ticket = lsp.ask(Query::Definition, total(MAIN)).unwrap();
        wait(&lsp, &ticket).unwrap();
    }
    let edited = MAIN.replace("total", "sum");
    lsp.sync([("src/main.rs", MAIN), ("src/other.rs", "fn x() {}")]);
    lsp.sync([("src/main.rs", edited.as_str())]);
    lsp.sync([("src/main.rs", edited.as_str())]);
    thread::sleep(Duration::from_millis(50));

    let methods = record.methods();
    let first_open = methods
        .iter()
        .position(|m| m == "textDocument/didOpen")
        .unwrap();
    let first_ask = methods
        .iter()
        .position(|m| m == "textDocument/definition")
        .unwrap();
    assert!(first_open < first_ask, "{methods:?}");
    assert_eq!(record.params("textDocument/didOpen").len(), 1);
    let changes = record.params("textDocument/didChange");
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0]["textDocument"]["version"], json!(2));
    assert_eq!(changes[0]["contentChanges"][0]["text"], json!(edited));
}

#[test]
fn typescript_and_tsx_share_one_server() {
    let (lsp, record) = fake::lsp(
        Path::new(ROOT),
        utf8(),
        answering("textDocument/definition", location("src/a.ts", 0)),
    );
    let text = "go();\n";
    for (language, path) in [
        (Language::TypeScript, "src/a.ts"),
        (Language::Tsx, "src/b.tsx"),
    ] {
        let ticket = lsp
            .ask(Query::Definition, symbol(language, path, text, 1, "go"))
            .unwrap();
        wait(&lsp, &ticket).unwrap();
    }
    assert_eq!(record.launches(), 1);
    assert_eq!(record.params("initialize").len(), 1);
    let languages: Vec<Value> = record
        .params("textDocument/didOpen")
        .iter()
        .map(|params| params["textDocument"]["languageId"].clone())
        .collect();
    assert_eq!(languages, [json!("typescript"), json!("typescriptreact")]);
}

fn progress(token: &str, value: Value) -> Message {
    Message::Notification(Notification::new(
        "$/progress".to_string(),
        json!({ "token": token, "value": value }),
    ))
}

#[test]
fn state_goes_from_off_to_loading_with_progress_to_ready() {
    let (release, gate) = crossbeam_channel::unbounded::<Vec<Message>>();
    let script: fake::Script = Arc::new(move |message| match message {
        Message::Request(r) if r.method == "initialize" => Reply::Then(
            gate.recv().unwrap(),
            Some(json!({ "capabilities": utf8() })),
        ),
        Message::Notification(n) if n.method == "initialized" => {
            Reply::Then(gate.recv().unwrap(), None)
        }
        Message::Request(r) if r.method == "textDocument/definition" => {
            Reply::Then(gate.recv().unwrap(), Some(location("src/cart.rs", 0)))
        }
        _ => Reply::Default,
    });
    let (lsp, _) = fake::lsp(Path::new(ROOT), utf8(), script);
    assert_eq!(lsp.state(Language::Rust), State::Off, "not started");
    assert_eq!(lsp.state(Language::Json), State::Off, "no server");
    lsp.warm(Language::Rust);
    assert_eq!(lsp.state(Language::Rust), State::Loading(None));

    release.send(Vec::new()).unwrap();
    release
        .send(vec![
            progress(
                "index",
                json!({ "kind": "begin", "title": "Indexing", "percentage": 10 }),
            ),
            progress("index", json!({ "kind": "report", "percentage": 62 })),
        ])
        .unwrap();
    assert!(eventually(
        || lsp.state(Language::Rust) == State::Loading(Some(62))
    ));

    release
        .send(vec![progress("index", json!({ "kind": "end" }))])
        .unwrap();
    let _ = lsp.ask(Query::Definition, total(MAIN));
    assert!(eventually(|| lsp.state(Language::Rust) == State::Ready));
}

fn eventually(mut check: impl FnMut() -> bool) -> bool {
    (0..400).any(|_| {
        let ok = check();
        if !ok {
            thread::sleep(Duration::from_millis(5));
        }
        ok
    })
}

#[test]
fn an_empty_answer_says_so_and_mentions_indexing_in_progress() {
    let script: fake::Script = Arc::new(|message| match message {
        Message::Notification(n) if n.method == "initialized" => Reply::Then(
            vec![progress(
                "t",
                json!({ "kind": "begin", "title": "Indexing" }),
            )],
            None,
        ),
        Message::Request(r) if r.method == "textDocument/definition" => Reply::Result(Value::Null),
        Message::Request(r) if r.method == "textDocument/prepareCallHierarchy" => {
            Reply::Result(json!([]))
        }
        _ => Reply::Default,
    });
    let (lsp, _) = fake::lsp(Path::new(ROOT), utf8(), script);
    let ticket = lsp.ask(Query::Definition, total(MAIN)).unwrap();
    assert_eq!(
        wait(&lsp, &ticket),
        Err("No definition found; rust-analyzer is still indexing".to_string())
    );

    let (lsp, _) = fake::lsp(
        Path::new(ROOT),
        utf8(),
        answering("textDocument/prepareCallHierarchy", json!([])),
    );
    let ticket = lsp.ask(Query::Callers, total(MAIN)).unwrap();
    assert_eq!(wait(&lsp, &ticket), Err("No callers found".to_string()));
}

#[test]
fn requests_from_the_server_get_replies() {
    let script: fake::Script = Arc::new(|message| match message {
        Message::Notification(n) if n.method == "initialized" => Reply::Then(
            vec![
                Message::Request(Request::new(
                    RequestId::from("config".to_string()),
                    "workspace/configuration".to_string(),
                    json!({ "items": [{ "section": "a" }, { "section": "b" }] }),
                )),
                Message::Request(Request::new(
                    RequestId::from("odd".to_string()),
                    "workspace/applyEdit".to_string(),
                    json!({}),
                )),
            ],
            None,
        ),
        _ => Reply::Default,
    });
    let (lsp, record) = fake::lsp(Path::new(ROOT), utf8(), script);
    lsp.warm(Language::Rust);
    assert!(eventually(|| {
        lsp.warm(Language::Rust);
        record
            .messages()
            .iter()
            .filter(|message| matches!(message, Message::Response(_)))
            .count()
            == 2
    }));
    let responses: Vec<_> = record
        .messages()
        .into_iter()
        .filter_map(|message| match message {
            Message::Response(response) => Some(response),
            _ => None,
        })
        .collect();
    assert_eq!(
        responses[0].response_result.as_ref().unwrap(),
        &json!([null, null])
    );
    assert!(responses[1].response_result.is_err());
}

fn rust_analyzer_options() -> Value {
    json!({
        "cargo": { "targetDir": true },
        "checkOnSave": false,
        "references": { "excludeImports": true }
    })
}

#[test]
fn rust_analyzer_starts_with_its_own_target_dir_and_no_check_on_save() {
    let (lsp, record) = fake::lsp(Path::new(ROOT), utf8(), answering("none", Value::Null));
    lsp.warm(Language::Rust);
    lsp.warm(Language::Go);
    assert!(eventually(|| record.params("initialize").len() == 2));
    let options: Vec<Option<Value>> = record
        .params("initialize")
        .iter()
        .map(|params| params.get("initializationOptions").cloned())
        .collect();
    assert!(options.contains(&Some(rust_analyzer_options())));
    assert!(options.contains(&None), "gopls gets no options");
}

#[test]
fn rust_analyzer_asking_for_its_configuration_gets_the_same_options() {
    let script: fake::Script = Arc::new(|message| match message {
        Message::Notification(n) if n.method == "initialized" => Reply::Then(
            vec![Message::Request(Request::new(
                RequestId::from("config".to_string()),
                "workspace/configuration".to_string(),
                json!({ "items": [{ "section": "rust-analyzer" }, { "section": "other" }] }),
            ))],
            None,
        ),
        _ => Reply::Default,
    });
    let (lsp, record) = fake::lsp(Path::new(ROOT), utf8(), script);
    lsp.warm(Language::Rust);
    let reply = || {
        record
            .messages()
            .into_iter()
            .find_map(|message| match message {
                Message::Response(response) => response.response_result.ok(),
                _ => None,
            })
    };
    assert!(eventually(|| {
        lsp.warm(Language::Rust);
        reply().is_some()
    }));
    assert_eq!(reply(), Some(json!([rust_analyzer_options(), null])));
}

#[test]
fn a_crash_fails_the_pending_query_and_the_next_one_restarts_the_server() {
    let (lsp, record) = fake::lsp(
        Path::new(ROOT),
        utf8(),
        Arc::new(|message| match message {
            Message::Request(r) if r.method == "textDocument/definition" => Reply::Hangup,
            _ => Reply::Default,
        }),
    );
    let ticket = lsp.ask(Query::Definition, total(MAIN)).unwrap();
    assert_eq!(
        wait(&lsp, &ticket),
        Err("rust-analyzer stopped; it restarts on the next request".to_string())
    );
    assert_eq!(lsp.state(Language::Rust), State::Failed);
    let _ = lsp.ask(Query::Definition, total(MAIN)).unwrap();
    assert_eq!(record.launches(), 2);
}

#[test]
fn a_server_that_exits_reports_its_last_error_line() {
    let table = ServerTable::parse(
        "[lsp.rust]\ncommand = [\"sh\", \"-c\", \"echo 'error: no toolchain' >&2\"]\n",
    );
    let lsp = Lsp::with_launcher(Path::new("/"), table, Box::new(super::transport::spawn));
    let ticket = lsp.ask(Query::Definition, total(MAIN)).unwrap();
    let answer = (0..400).find_map(|_| {
        thread::sleep(Duration::from_millis(5));
        lsp.answer(&ticket)
    });
    assert_eq!(
        answer,
        Some(Err("sh stopped: error: no toolchain".to_string()))
    );
}

#[test]
fn a_missing_or_disabled_server_fails_the_ask() {
    let table = ServerTable::parse(
        "[lsp.go]\ncommand = [\"deltoids-no-such-server\"]\n[lsp.python]\nenabled = false\n",
    );
    let lsp = Lsp::with_launcher(Path::new(ROOT), table, Box::new(super::transport::spawn));
    let go = lsp.ask(
        Query::Definition,
        symbol(Language::Go, "a.go", "go()\n", 1, "go"),
    );
    assert_eq!(
        go.unwrap_err(),
        "No language server for Go (deltoids-no-such-server not found)"
    );
    assert_eq!(
        lsp.state(Language::Go),
        State::Off,
        "a missing program is off"
    );
    let python = lsp.ask(
        Query::Definition,
        symbol(Language::Python, "a.py", "go()\n", 1, "go"),
    );
    assert_eq!(python.unwrap_err(), "No language server for Python");
    assert!(!lsp.serves(Language::Python));
}

#[test]
fn a_cancelled_query_is_cancelled_at_the_server_and_never_answers() {
    let (lsp, record) = fake::lsp(
        Path::new(ROOT),
        utf8(),
        Arc::new(|message| match message {
            Message::Request(r) if r.method == "textDocument/definition" => Reply::Silent,
            _ => Reply::Default,
        }),
    );
    let ticket = lsp.ask(Query::Definition, total(MAIN)).unwrap();
    assert!(eventually(|| {
        lsp.state(Language::Rust);
        !record.params("textDocument/definition").is_empty()
    }));
    let id = record
        .messages()
        .into_iter()
        .find_map(|message| match message {
            Message::Request(r) if r.method == "textDocument/definition" => Some(r.id),
            _ => None,
        })
        .unwrap();
    let probe = Ticket(ticket.0);
    lsp.cancel(ticket);
    assert!(eventually(|| !record.params("$/cancelRequest").is_empty()));
    assert_eq!(record.params("$/cancelRequest")[0]["id"], json!(id));
    thread::sleep(Duration::from_millis(30));
    assert_eq!(lsp.answer(&probe), None);
}

#[test]
fn dropping_the_last_handle_shuts_the_servers_down() {
    let (lsp, record) = fake::lsp(Path::new(ROOT), utf8(), Arc::new(|_| Reply::Default));
    lsp.warm(Language::Rust);
    assert!(eventually(|| lsp.state(Language::Rust) == State::Ready));
    let clone = lsp.clone();
    drop(lsp);
    assert!(record.params("shutdown").is_empty(), "a clone is alive");
    drop(clone);
    assert!(eventually(|| record
        .methods()
        .ends_with(&["shutdown".to_string(), "exit".to_string()])));
}

fn unregister(request: &str, id: &str) -> Message {
    Message::Request(Request::new(
        RequestId::from(request.to_string()),
        "client/unregisterCapability".to_string(),
        json!({ "unregisterations": [{ "id": id, "method": "workspace/didChangeWatchedFiles" }] }),
    ))
}

/// A watcher for `pattern` under `root`, written as rust-analyzer
/// writes them: an absolute glob.
fn glob(root: &Path, pattern: &str) -> Value {
    json!({ "globPattern": format!("{}/{pattern}", root.display()) })
}

/// Every change the servers were told about: file URI and change type.
fn heard(record: &fake::Record) -> Vec<(String, u64)> {
    record
        .params("workspace/didChangeWatchedFiles")
        .iter()
        .flat_map(|params| params["changes"].as_array().cloned().unwrap_or_default())
        .map(|change| {
            (
                change["uri"].as_str().unwrap().to_string(),
                change["type"].as_u64().unwrap(),
            )
        })
        .collect()
}

fn file_uri(path: &Path) -> String {
    url::Url::from_file_path(path).unwrap().to_string()
}

fn event(kind: EventKind, path: &Path) -> notify::Event {
    notify::Event::new(kind).add_path(path.to_path_buf())
}

/// A workdir whose Rust server registered `watchers` once initialized.
fn watched_workdir(watchers: impl Fn(&Path) -> Value) -> (tempfile::TempDir, Lsp, fake::Record) {
    let dir = tempfile::tempdir().unwrap();
    let (lsp, record) = fake::lsp(dir.path(), utf8(), fake::watching(watchers(dir.path())));
    lsp.warm(Language::Rust);
    assert!(eventually(|| {
        lsp.poll();
        record.replied("register")
    }));
    (dir, lsp, record)
}

#[test]
fn files_changed_on_disk_reach_the_server_that_watches_them() {
    let (dir, lsp, record) = watched_workdir(|root| json!([glob(root, "**/*.rs")]));
    let advertised = &record.params("initialize")[0]["capabilities"]["workspace"];
    assert_eq!(
        advertised["didChangeWatchedFiles"]["dynamicRegistration"],
        json!(true)
    );

    let files = lsp.file_events();
    let path = dir.path().join("src/new.rs");
    let notes = dir.path().join("notes.txt");
    let folder = dir.path().join("src/generated.rs");
    fs::create_dir_all(&folder).unwrap();
    fs::write(dir.path().join("src/read.rs"), "").unwrap();
    files.record(&event(
        EventKind::Access(AccessKind::Open(AccessMode::Read)),
        &dir.path().join("src/read.rs"),
    ));
    fs::write(&path, "fn a() {}\n").unwrap();
    fs::write(&notes, "").unwrap();
    for created in [&path, &notes, &folder] {
        files.record(&event(EventKind::Create(CreateKind::Any), created));
    }
    lsp.poll();
    fs::write(&path, "fn b() {}\n").unwrap();
    files.record(&event(EventKind::Modify(ModifyKind::Any), &path));
    lsp.poll();
    fs::remove_file(&path).unwrap();
    files.record(&event(EventKind::Remove(RemoveKind::Any), &path));
    lsp.poll();

    assert!(eventually(|| heard(&record).len() >= 3));
    let uri = file_uri(&path);
    assert_eq!(
        heard(&record),
        [(uri.clone(), 1), (uri.clone(), 2), (uri, 3)],
        "created, changed, deleted; never a read, the text file, or the folder"
    );
}

#[test]
fn a_watcher_hears_only_the_kinds_of_change_it_asked_for() {
    let (dir, lsp, record) = watched_workdir(|root| {
        json!([
            { "globPattern": format!("{}/**/*.rs", root.display()), "kind": 4 },
            { "globPattern": format!("{}/**/*.md", root.display()) }
        ])
    });
    let files = lsp.file_events();
    let (code, notes) = (dir.path().join("gone.rs"), dir.path().join("notes.md"));
    for path in [&code, &notes] {
        fs::write(path, "").unwrap();
        files.record(&event(EventKind::Create(CreateKind::File), path));
    }
    lsp.poll();
    fs::remove_file(&code).unwrap();
    files.record(&event(EventKind::Remove(RemoveKind::File), &code));
    lsp.poll();

    assert!(eventually(|| heard(&record).len() >= 2));
    assert_eq!(
        heard(&record),
        [(file_uri(&notes), 1), (file_uri(&code), 3)],
        "the .rs watcher hears deletions only; the .md one hears all"
    );
}

#[test]
fn a_registration_replaces_the_one_with_its_id_and_unregistering_ends_it() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let (release, gate) = crossbeam_channel::unbounded::<Vec<Message>>();
    let rust = glob(&root, "**/*.rs");
    let script: fake::Script = Arc::new(move |message| match message {
        Message::Notification(n) if n.method == "initialized" => Reply::Then(
            vec![fake::register_watchers(
                "first",
                "watch",
                json!([rust.clone()]),
            )],
            None,
        ),
        Message::Request(r) if r.method == "textDocument/definition" => {
            Reply::Then(gate.recv().unwrap(), Some(Value::Null))
        }
        _ => Reply::Default,
    });
    let (lsp, record) = fake::lsp(&root, utf8(), script);
    let files = lsp.file_events();
    // Whatever the server sends before it answers is handled once the
    // answer is in.
    let round_trip = |messages: Vec<Message>| {
        release.send(messages).unwrap();
        let ticket = lsp.ask(Query::Definition, total(MAIN)).unwrap();
        let _ = wait(&lsp, &ticket);
    };

    round_trip(vec![fake::register_watchers(
        "second",
        "watch",
        json!([glob(&root, "**/*.md")]),
    )]);
    let (code, notes) = (root.join("a.rs"), root.join("a.md"));
    for path in [&code, &notes] {
        fs::write(path, "").unwrap();
        files.record(&event(EventKind::Create(CreateKind::File), path));
    }
    round_trip(vec![unregister("third", "watch")]);
    files.record(&event(EventKind::Modify(ModifyKind::Any), &notes));
    round_trip(Vec::new());

    assert_eq!(heard(&record), [(file_uri(&notes), 1)]);
}

#[test]
fn each_server_hears_only_about_the_files_it_watches() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let started = AtomicUsize::new(0);
    let patterns = [glob(&root, "**/*.rs"), glob(&root, "**/*.go")];
    let script: fake::Script = Arc::new(move |message| match message {
        Message::Notification(n) if n.method == "initialized" => {
            let nth = started.fetch_add(1, Ordering::SeqCst);
            Reply::Then(
                vec![fake::register_watchers(
                    &format!("register-{nth}"),
                    "watch",
                    json!([patterns[nth].clone()]),
                )],
                None,
            )
        }
        _ => Reply::Default,
    });
    let (lsp, record) = fake::lsp(&root, utf8(), script);
    for (language, request) in [(Language::Rust, "register-0"), (Language::Go, "register-1")] {
        lsp.warm(language);
        assert!(eventually(|| {
            lsp.poll();
            record.replied(request)
        }));
    }
    let files = lsp.file_events();
    let (code, go) = (root.join("a.rs"), root.join("b.go"));
    for path in [&code, &go] {
        fs::write(path, "").unwrap();
        files.record(&event(EventKind::Create(CreateKind::File), path));
    }
    lsp.poll();

    let told = || record.params("workspace/didChangeWatchedFiles");
    assert!(eventually(|| told().len() >= 2));
    let mut told: Vec<Vec<String>> = told()
        .iter()
        .map(|params| {
            params["changes"]
                .as_array()
                .unwrap()
                .iter()
                .map(|change| change["uri"].as_str().unwrap().to_string())
                .collect()
        })
        .collect();
    told.sort();
    assert_eq!(told, [vec![file_uri(&code)], vec![file_uri(&go)]]);
}

#[test]
fn a_change_on_disk_reaches_the_server_before_the_lookup_that_follows() {
    let (dir, lsp, record) = watched_workdir(|root| json!([glob(root, "**/*.rs")]));
    let cart = dir.path().join("src/cart.rs");
    fs::create_dir_all(cart.parent().unwrap()).unwrap();
    fs::write(&cart, "pub fn total() {}\n").unwrap();
    lsp.file_events()
        .record(&event(EventKind::Modify(ModifyKind::Any), &cart));
    let ticket = lsp.ask(Query::Definition, total(MAIN)).unwrap();
    let _ = wait(&lsp, &ticket);

    let methods = record.methods();
    let told = methods
        .iter()
        .position(|m| m == "workspace/didChangeWatchedFiles");
    let asked = methods.iter().position(|m| m == "textDocument/definition");
    assert!(told.is_some() && told < asked, "{methods:?}");
}

#[test]
fn missed_changes_restart_only_the_servers_that_watch_files() {
    let dir = tempfile::tempdir().unwrap();
    let rust = glob(dir.path(), "**/*.rs");
    let started = AtomicUsize::new(0);
    let script: fake::Script = Arc::new(move |message| match message {
        Message::Notification(n)
            if n.method == "initialized" && started.fetch_add(1, Ordering::SeqCst) == 0 =>
        {
            Reply::Then(
                vec![fake::register_watchers(
                    "register",
                    "watch",
                    json!([rust.clone()]),
                )],
                None,
            )
        }
        Message::Request(r) if r.method == "textDocument/definition" => Reply::Silent,
        _ => Reply::Default,
    });
    let (lsp, record) = fake::lsp(dir.path(), utf8(), script);
    let files = lsp.file_events();
    files.lost();
    lsp.warm(Language::Rust);
    assert!(eventually(|| {
        lsp.poll();
        record.replied("register")
    }));
    lsp.warm(Language::Go);
    assert!(eventually(|| lsp.state(Language::Go) == State::Ready));
    assert_eq!(record.launches(), 2, "nothing watched files before");
    let ticket = lsp.ask(Query::Definition, total(MAIN)).unwrap();
    assert!(eventually(|| {
        lsp.poll();
        !record.params("textDocument/definition").is_empty()
    }));

    files.lost();
    lsp.poll();
    assert_eq!(record.launches(), 3, "rust-analyzer again; gopls kept");
    assert_eq!(
        lsp.answer(&ticket),
        Some(Err(
            "rust-analyzer restarted to catch up with changes on disk; ask again".to_string()
        ))
    );
}

#[test]
#[ignore = "needs rust-analyzer (on PATH, or its path in RUST_ANALYZER); run with --ignored"]
fn rust_analyzer_finds_a_definition_in_another_file() {
    let dir = tempfile::tempdir().unwrap();
    let write = |path: &str, text: &str| {
        let path = dir.path().join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    };
    write(
        "Cargo.toml",
        "[package]\nname = \"shop\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    write("src/cart.rs", "pub fn total() -> u32 {\n    42\n}\n");
    write("src/main.rs", MAIN);
    let program = std::env::var("RUST_ANALYZER").unwrap_or("rust-analyzer".to_string());
    let table = ServerTable::parse(&format!("[lsp.rust]\ncommand = [{program:?}]\n"));
    let lsp = Lsp::with_launcher(dir.path(), table, Box::new(super::transport::spawn));
    let answer = |ticket: &Ticket| loop {
        if let Some(answer) = lsp.answer(ticket) {
            return places(answer);
        }
        thread::sleep(Duration::from_millis(20));
    };
    let ask_until = |expected: Vec<Found>| {
        let deadline = std::time::Instant::now() + Duration::from_secs(120);
        let mut last = Err(String::new());
        while std::time::Instant::now() < deadline {
            let ticket = lsp.ask(Query::Definition, total(MAIN)).unwrap();
            last = answer(&ticket);
            if last.as_ref() == Ok(&expected) {
                break;
            }
            thread::sleep(Duration::from_millis(500));
        }
        assert_eq!(last, Ok(expected));
    };
    ask_until(vec![found("src/cart.rs", 1, None)]);
    // The server makes its target directory in the background.
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    while !dir.path().join("target/rust-analyzer").is_dir() && std::time::Instant::now() < deadline
    {
        thread::sleep(Duration::from_millis(100));
    }
    assert!(dir.path().join("target/rust-analyzer").is_dir());
    assert!(!dir.path().join("target/debug").exists());

    // deltoids never opened cart.rs: only the watched-files channel can
    // tell the server it moved.
    write("src/cart.rs", "\n\npub fn total() -> u32 {\n    42\n}\n");
    lsp.file_events().record(&event(
        EventKind::Modify(ModifyKind::Any),
        &dir.path().join("src/cart.rs"),
    ));
    ask_until(vec![found("src/cart.rs", 3, None)]);
}

const MODES_MAIN: &str = "\
mod modes;
use modes::{Mode, Stage, Workspace, LIMIT};

fn shell(mode: &mut impl Mode) {
    mode.reload();
}

fn direct(w: &mut Workspace) {
    w.reload();
}

const PICK: fn(&mut Workspace) = Workspace::reload_inherent;

fn main() {
    shell(&mut Workspace);
    direct(&mut Workspace);
    let f = <Workspace as Mode>::reload;
    f(&mut Workspace);
    let _ = PICK;
    let _ = Stage::Declared;
    let _ = LIMIT + 1;
    let _ = modes::Mock;
}
";

const MODES: &str = "\
pub trait Mode {
    fn reload(&mut self);
}

pub struct Workspace;
pub struct Mock;

pub const LIMIT: u32 = 9;

pub enum Stage {
    Declared,
}

impl Workspace {
    pub fn reload_inherent(&mut self) {}
}

impl Mode for Workspace {
    fn reload(&mut self) {}
}

impl Mode for Mock {
    fn reload(&mut self) {}
}

pub fn mock_call(m: &mut Mock) {
    m.reload();
}
";

#[test]
#[ignore = "needs rust-analyzer (on PATH, or its path in RUST_ANALYZER); run with --ignored"]
fn rust_analyzer_lists_every_use_of_any_symbol() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(
        dir.path().join("Cargo.toml"),
        "[package]\nname = \"modes\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(dir.path().join("src/main.rs"), MODES_MAIN).unwrap();
    fs::write(dir.path().join("src/modes.rs"), MODES).unwrap();
    let program = std::env::var("RUST_ANALYZER").unwrap_or("rust-analyzer".to_string());
    let table = ServerTable::parse(&format!("[lsp.rust]\ncommand = [{program:?}]\n"));
    let lsp = Lsp::with_launcher(dir.path(), table, Box::new(super::transport::spawn));
    let uses = |line: usize, name: &str| -> Vec<(String, usize)> {
        let ticket = lsp
            .ask(
                Query::Callers,
                symbol(Language::Rust, "src/modes.rs", MODES, line, name),
            )
            .unwrap();
        let answer = loop {
            if let Some(answer) = lsp.answer(&ticket) {
                break answer;
            }
            thread::sleep(Duration::from_millis(20));
        };
        places(answer)
            .unwrap_or_default()
            .into_iter()
            .map(|found| (found.path, found.line))
            .collect()
    };
    let main = |lines: &[usize]| -> Vec<(String, usize)> {
        lines
            .iter()
            .map(|&line| ("src/main.rs".to_string(), line))
            .collect()
    };

    let impl_reload = main(&[5, 9, 17]);
    let deadline = std::time::Instant::now() + Duration::from_secs(120);
    let mut last = Vec::new();
    while std::time::Instant::now() < deadline {
        last = uses(19, "reload");
        if last == impl_reload {
            break;
        }
        thread::sleep(Duration::from_millis(500));
    }
    assert_eq!(last, impl_reload, "the impl's reload");

    let mut trait_reload = main(&[5, 9, 17]);
    trait_reload.push(("src/modes.rs".to_string(), 27));
    assert_eq!(uses(2, "reload"), trait_reload, "the trait's reload");
    assert_eq!(
        uses(15, "reload_inherent"),
        main(&[12]),
        "a function in a const"
    );
    assert_eq!(
        uses(8, "LIMIT"),
        main(&[21]),
        "a constant, without its import"
    );
    assert_eq!(uses(11, "Declared"), main(&[20]), "an enum variant");
    let workspace = uses(5, "Workspace");
    assert!(
        !workspace.contains(&("src/main.rs".to_string(), 2)),
        "{workspace:?}"
    );
    assert!(
        workspace.contains(&("src/main.rs".to_string(), 8)),
        "{workspace:?}"
    );
}

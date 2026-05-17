//! `deltoids hashedit` — agent edit tool using hashline anchors.

use std::io::{self, IsTerminal, Read};
use std::process::ExitCode;

use clap::Args as ClapArgs;

use crate::{
    ErrorResponse, HashEditRequest, execute_hash_edit_request_with_trace, trace_store::TraceStore,
};

const OVERVIEW: &str = r#"CLI for agents to edit files via hashline anchors.

Anchors come from `deltoids hashread`. Each line is shown as
`LINEhh|TEXT`; the anchor token is the `LINEhh` part (e.g. `42sr`). The
2-char hash is a content fingerprint computed from the line — its only
job is to detect "the file changed since you last read it" at apply
time.

Input (JSON on stdin):
- reason: why this change is being made. Required. Must not be empty.
- path: UTF-8 text file to edit. Must exist and be a file.
- edits: one or more operations. Each op is one of:

  {"op": "replace",
   "reason": "...", "pos": "LINEhh", "end": "LINEhh"?, "lines": [...]}

  {"op": "insert_before",
   "reason": "...", "pos": "LINEhh" | "BOF", "lines": [...]}

  {"op": "insert_after",
   "reason": "...", "pos": "LINEhh" | "EOF", "lines": [...]}

  {"op": "delete",
   "reason": "...", "pos": "LINEhh", "end": "LINEhh"?}

Rules:
- Every anchor is validated against the current file before any change.
  A single stale anchor rejects the whole batch and the file is untouched.
- Multiple ops are applied bottom-up against the *original* file. Do not
  shift later anchors after earlier ops.
- Op regions must not overlap.
- Each edit must carry a non-empty reason.
- If you pass a trace id, it must reference an existing trace; omit it
  to start a new trace.

On stale anchor, the error message reprints the affected region with
*fresh* anchors and `*` markers on the mismatched lines, so you can
retry without re-reading the whole file.

Example:
printf '%s' '{
  "reason": "Rename variable",
  "path": "src/app.ts",
  "edits": [
    {
      "op": "replace",
      "reason": "Rename x to count",
      "pos": "4sr",
      "lines": ["const count = 1;"]
    }
  ]
}' | deltoids hashedit

To review traces, run `deltoids traces`.

Output:
- Success goes to stdout as JSON.
- Failure goes to stderr as JSON and exits non-zero.
"#;

#[derive(Debug, Default, ClapArgs)]
#[command(after_help = OVERVIEW)]
pub struct Args {
    pub trace_id: Option<String>,
}

pub fn run(args: Args) -> ExitCode {
    match run_inner(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(response) => {
            eprintln!(
                "{}",
                serde_json::to_string(&response).expect("error response should serialize")
            );
            ExitCode::from(1)
        }
    }
}

fn run_inner(args: Args) -> Result<(), ErrorResponse> {
    let mut stdin = io::stdin();
    if stdin.is_terminal() {
        print_overview();
        return Ok(());
    }

    let mut input = String::new();
    stdin
        .read_to_string(&mut input)
        .map_err(|err| simple_error(format!("Failed to read stdin: {err}")))?;

    if should_show_overview(false, &input) {
        print_overview();
        return Ok(());
    }

    let request: HashEditRequest = serde_json::from_str(&input)
        .map_err(|err| simple_error(format!("Invalid request JSON: {err}")))?;

    let store = TraceStore::from_env().map_err(simple_error)?;
    let response = execute_hash_edit_request_with_trace(&store, request, args.trace_id.as_deref())
        .map_err(|error| ErrorResponse {
            ok: false,
            error: error.error,
            trace_id: (!error.trace_id.is_empty()).then_some(error.trace_id),
            message: (!error.message.is_empty()).then_some(error.message),
        })?;
    println!(
        "{}",
        serde_json::to_string(&response).expect("success response should serialize")
    );
    Ok(())
}

fn simple_error(error: String) -> ErrorResponse {
    ErrorResponse {
        ok: false,
        error,
        trace_id: None,
        message: None,
    }
}

fn print_overview() {
    println!("{OVERVIEW}");
}

fn should_show_overview(stdin_is_terminal: bool, input: &str) -> bool {
    stdin_is_terminal || input.trim().is_empty()
}

#[cfg(test)]
mod tests {
    #[test]
    fn shows_overview_when_stdin_is_a_terminal() {
        assert!(super::should_show_overview(true, ""));
    }

    #[test]
    fn shows_overview_when_stdin_is_whitespace_only() {
        assert!(super::should_show_overview(false, " \n\t "));
    }

    #[test]
    fn does_not_show_overview_for_non_empty_piped_input() {
        assert!(!super::should_show_overview(
            false,
            "{\"reason\":\"r\",\"path\":\"x\",\"edits\":[]}"
        ));
    }
}

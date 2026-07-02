//! `deltoids edit` — agent edit tool, appends to a trace.

use std::io::{self, IsTerminal, Read};
use std::process::ExitCode;

use clap::Args as ClapArgs;

use crate::{EditRequest, ErrorResponse, execute_request_with_trace, trace_store::TraceStore};

const OVERVIEW: &str = r#"CLI for agents to edit files.

Input (a single exact replacement):
- reason: why this change is being made. Required. Must not be empty.
- path: UTF-8 text file to edit. Must exist and be a file.
- oldText: exact text to replace. Required. Must not be empty.
- newText: replacement text.

Rules:
- oldText must match exactly, including whitespace and newlines.
- oldText must match exactly once in the file.
- To make several changes, call `edit` once per change. Each call
  matches against the file's current text, so target text as it exists
  after any earlier edit.
- Unknown JSON fields are rejected.
- If you pass a trace id, it must reference an existing trace.
- Omit the trace id to start a new trace.
- If the path does not exist, the error is: Path does not exist: <path>
- If the path is not a file, the error is: Path is not a file: <path>
- If the edit fails, nothing is written.

Examples:
printf '%s' '{
  "reason": "Rename x to count to reflect what it tracks",
  "path": "src/app.ts",
  "oldText": "const x = 1;",
  "newText": "const count = 1;"
}' | deltoids edit

deltoids edit [trace-id] --path src/app.ts --reason "Rename x" --old "const x = 1;" --new "const count = 1;"

To review traces, run `deltoids traces`.

Output:
- Success goes to stdout as JSON.
- Failure goes to stderr as JSON and exits non-zero.
"#;

#[derive(Debug, Default, ClapArgs)]
#[command(after_help = OVERVIEW)]
pub struct Args {
    pub trace_id: Option<String>,
    #[arg(long)]
    pub path: Option<String>,
    #[arg(long)]
    pub reason: Option<String>,
    #[arg(long = "old")]
    pub old_text: Option<String>,
    #[arg(long = "new")]
    pub new_text: Option<String>,
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
    let request = if uses_shorthand(&args) {
        edit_request_from_shorthand(&args).map_err(simple_error)?
    } else {
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

        serde_json::from_str(&input)
            .map_err(|err| simple_error(format!("Invalid request JSON: {err}")))?
    };

    let store = TraceStore::from_env().map_err(simple_error)?;
    let response =
        execute_request_with_trace(&store, request, args.trace_id.as_deref()).map_err(|error| {
            ErrorResponse {
                ok: false,
                error: error.error,
                trace_id: (!error.trace_id.is_empty()).then_some(error.trace_id),
                message: (!error.message.is_empty()).then_some(error.message),
            }
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

fn uses_shorthand(args: &Args) -> bool {
    args.path.is_some()
        || args.reason.is_some()
        || args.old_text.is_some()
        || args.new_text.is_some()
}

fn edit_request_from_shorthand(args: &Args) -> Result<EditRequest, String> {
    let path = args
        .path
        .clone()
        .ok_or_else(|| "--path, --reason, --old, and --new are required together".to_string())?;
    let reason = args
        .reason
        .clone()
        .ok_or_else(|| "--path, --reason, --old, and --new are required together".to_string())?;
    let old_text = args
        .old_text
        .clone()
        .ok_or_else(|| "--path, --reason, --old, and --new are required together".to_string())?;
    let new_text = args
        .new_text
        .clone()
        .ok_or_else(|| "--path, --reason, --old, and --new are required together".to_string())?;

    Ok(EditRequest {
        reason,
        path,
        old_text,
        new_text,
    })
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
        assert!(!super::should_show_overview(false, "{\"reason\":\"x\"}"));
    }
}

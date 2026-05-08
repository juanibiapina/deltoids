//! `deltoids edit` — agent edit tool, appends to a trace.

use std::io::{self, IsTerminal, Read};
use std::process::ExitCode;

use clap::Args as ClapArgs;

use crate::{
    EditRequest, ErrorResponse, TextEdit, execute_request_with_trace, trace_store::TraceStore,
};

const OVERVIEW: &str = r#"CLI for agents to edit files.

Input:
- summary: short description of the change. Required. Must not be empty.
- path: UTF-8 text file to edit. Must exist and be a file.
- edits: one or more replacements.

Each edit must use:
- summary: short description of that edit. Required. Must not be empty.
- oldText
- newText

Rules:
- oldText must match exactly, including whitespace and newlines.
- Each oldText must match exactly once in the original file.
- All edits are matched against the original file, not after earlier edits are applied.
- Edit regions must not overlap.
- Unknown JSON fields are rejected.
- If you pass a trace id, it must be an existing ULID trace id.
- Omit the trace id to start a new trace.
- If the path does not exist, the error is: Path does not exist: <path>
- If the path is not a file, the error is: Path is not a file: <path>
- If any edit fails, nothing is written.

Examples:
printf '%s' '{
  "summary": "Rename variable",
  "path": "src/app.ts",
  "edits": [
    {
      "summary": "Rename x to count",
      "oldText": "const x = 1;",
      "newText": "const count = 1;"
    }
  ]
}' | deltoids edit

deltoids edit [trace-id] --path src/app.ts --summary "Rename x" --old "const x = 1;" --new "const count = 1;"

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
    pub summary: Option<String>,
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
        || args.summary.is_some()
        || args.old_text.is_some()
        || args.new_text.is_some()
}

fn edit_request_from_shorthand(args: &Args) -> Result<EditRequest, String> {
    let path = args
        .path
        .clone()
        .ok_or_else(|| "--path, --summary, --old, and --new are required together".to_string())?;
    let summary = args
        .summary
        .clone()
        .ok_or_else(|| "--path, --summary, --old, and --new are required together".to_string())?;
    let old_text = args
        .old_text
        .clone()
        .ok_or_else(|| "--path, --summary, --old, and --new are required together".to_string())?;
    let new_text = args
        .new_text
        .clone()
        .ok_or_else(|| "--path, --summary, --old, and --new are required together".to_string())?;

    Ok(EditRequest {
        summary: summary.clone(),
        path,
        edits: vec![TextEdit {
            summary,
            old_text,
            new_text,
        }],
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
        assert!(!super::should_show_overview(false, "{\"summary\":\"x\"}"));
    }
}

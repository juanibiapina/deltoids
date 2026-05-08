//! `deltoids write` — agent write tool, appends to a trace.

use std::io::{self, IsTerminal, Read};
use std::process::ExitCode;

use clap::Args as ClapArgs;

use crate::{
    ErrorResponse, WriteRequest, execute_write_request_with_trace, trace_store::TraceStore,
};

const OVERVIEW: &str = r#"CLI for agents to rewrite files.

Input:
- reason: why this file is being written. Required. Must not be empty.
- path: UTF-8 text file to write.
- content: full file contents to write.

Rules:
- Unknown JSON fields are rejected.
- If you pass a trace id, it must reference an existing trace.
- Omit the trace id to start a new trace.
- If the path exists, it must be a file.
- Parent directories are created as needed.

Examples:
printf '%s' '{
  "reason": "Bump config to v2",
  "path": "config.json",
  "content": "{\n  \"version\": 2\n}\n"
}' | deltoids write

deltoids write [trace-id] --path config.json --reason "Bump config to v2" < config.json.new

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
        write_request_from_shorthand(&args).map_err(simple_error)?
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
    let response = execute_write_request_with_trace(&store, request, args.trace_id.as_deref())
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

fn uses_shorthand(args: &Args) -> bool {
    args.path.is_some() || args.reason.is_some()
}

fn write_request_from_shorthand(args: &Args) -> Result<WriteRequest, String> {
    let path = args
        .path
        .clone()
        .ok_or_else(|| "--path and --reason are required together".to_string())?;
    let reason = args
        .reason
        .clone()
        .ok_or_else(|| "--path and --reason are required together".to_string())?;

    let mut content = String::new();
    io::stdin()
        .read_to_string(&mut content)
        .map_err(|err| format!("Failed to read stdin: {err}"))?;

    Ok(WriteRequest {
        reason,
        path,
        content,
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

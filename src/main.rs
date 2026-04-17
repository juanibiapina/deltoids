mod highlight;
mod review;

use std::env;
use std::io::{self, IsTerminal, Read};
use std::process::ExitCode;

use clap::Parser;
use edit::{
    EditRequest, ErrorResponse, TextEdit, execute_request_with_trace,
    list_traces_for_current_directory, read_history_entries,
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
}' | edit

edit [trace-id] --path src/app.ts --summary "Rename x" --old "const x = 1;" --new "const count = 1;"
edit traces list
edit traces list <trace-id>
edit traces show <trace-id> <index>
edit traces review <trace-id>

Output:
- Success goes to stdout as JSON.
- Failure goes to stderr as JSON and exits non-zero.
"#;

#[derive(Debug, Parser)]
#[command(
    name = "edit",
    about = "CLI for agents to edit files.",
    after_help = OVERVIEW
)]
struct Cli {
    trace_id: Option<String>,
    #[arg(long)]
    path: Option<String>,
    #[arg(long)]
    summary: Option<String>,
    #[arg(long = "old")]
    old_text: Option<String>,
    #[arg(long = "new")]
    new_text: Option<String>,
}

fn main() -> ExitCode {
    match run() {
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

fn run() -> Result<(), ErrorResponse> {
    let raw_args = env::args().skip(1).collect::<Vec<_>>();
    if let Some(result) = maybe_run_trace_command(&raw_args) {
        return result.map_err(simple_error);
    }

    let cli = Cli::parse();

    let request = if uses_shorthand(&cli) {
        edit_request_from_shorthand(&cli).map_err(simple_error)?
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

    let response =
        execute_request_with_trace(request, cli.trace_id.as_deref()).map_err(|error| {
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

fn maybe_run_trace_command(args: &[String]) -> Option<Result<(), String>> {
    match args {
        [traces, list] if traces == "traces" && list == "list" => Some(run_traces_list()),
        [traces, list, trace_id] if traces == "traces" && list == "list" => {
            Some(run_history_list(trace_id))
        }
        [traces, show, trace_id, index] if traces == "traces" && show == "show" => {
            Some(run_history_show(trace_id, index))
        }
        [traces, review, trace_id] if traces == "traces" && review == "review" => {
            Some(run_history_review(trace_id))
        }
        [traces, ..] if traces == "traces" => Some(Err(trace_usage().to_string())),
        [trace, ..] if trace == "trace" => Some(Err(trace_usage().to_string())),
        [history, ..] if history == "history" => Some(Err(trace_usage().to_string())),
        _ => None,
    }
}

fn trace_usage() -> &'static str {
    "Usage: edit traces list\n       edit traces list <trace-id>\n       edit traces show <trace-id> <index>\n       edit traces review <trace-id>"
}

fn run_traces_list() -> Result<(), String> {
    let traces = list_traces_for_current_directory()?;
    for trace in traces {
        println!(
            "{}\t{}\t{}\t{}\t{}\t{}",
            trace.trace_id,
            trace.entry_count,
            trace.last_timestamp,
            trace.last_tool,
            trace.last_path,
            trace.last_summary
        );
    }
    Ok(())
}

fn run_history_list(trace_id: &str) -> Result<(), String> {
    let entries = read_history_entries(trace_id)?;
    for (index, entry) in entries.iter().enumerate() {
        let status = if entry.ok { "ok" } else { "fail" };
        println!(
            "{}\t{}\t{}\t{}\t{}",
            index + 1,
            entry.tool,
            status,
            entry.path,
            entry.summary
        );
    }
    Ok(())
}

fn run_history_show(trace_id: &str, index: &str) -> Result<(), String> {
    let entries = read_history_entries(trace_id)?;
    let index = index
        .parse::<usize>()
        .map_err(|_| format!("Invalid trace entry index: {index}"))?;
    if index == 0 || index > entries.len() {
        return Err(format!("Trace entry index out of range: {index}"));
    }

    let entry = &entries[index - 1];
    println!("tool: {}", entry.tool);
    println!("summary: {}", entry.summary);

    if entry.tool == "edit" {
        for (edit_index, edit) in entry.edits.iter().enumerate() {
            println!("edit {}: {}", edit_index + 1, edit.summary);
        }
    }

    if entry.ok {
        if let Some(diff) = &entry.diff {
            println!("diff:");
            print!("{diff}");
        }
    } else if let Some(error) = &entry.error {
        println!("error: {error}");
    }

    Ok(())
}

fn run_history_review(trace_id: &str) -> Result<(), String> {
    let entries = read_history_entries(trace_id)?;
    if entries.is_empty() {
        return Err(format!("Trace has no entries: {trace_id}"));
    }

    review::run(&entries)
}

fn simple_error(error: String) -> ErrorResponse {
    ErrorResponse {
        ok: false,
        error,
        trace_id: None,
        message: None,
    }
}

fn uses_shorthand(cli: &Cli) -> bool {
    cli.path.is_some() || cli.summary.is_some() || cli.old_text.is_some() || cli.new_text.is_some()
}

fn edit_request_from_shorthand(cli: &Cli) -> Result<EditRequest, String> {
    let path = cli
        .path
        .clone()
        .ok_or_else(|| "--path, --summary, --old, and --new are required together".to_string())?;
    let summary = cli
        .summary
        .clone()
        .ok_or_else(|| "--path, --summary, --old, and --new are required together".to_string())?;
    let old_text = cli
        .old_text
        .clone()
        .ok_or_else(|| "--path, --summary, --old, and --new are required together".to_string())?;
    let new_text = cli
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

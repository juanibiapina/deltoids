//! `deltoids record` — import a Kao capture archive as one trace entry.

use std::io::{self, Read};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::Args as ClapArgs;

use crate::{ErrorResponse, RecordRequest, record_capture, trace_store::TraceStore};

const OVERVIEW: &str = r#"Record the file changes of a command captured by Kao.

Run a command through Kao, then hand its capture archive to this tool:

    kao run -- bash -c 'cargo fmt' 3>/tmp/capture.tar
    printf '%s' '{"tool": "bash", "command": "cargo fmt"}' \
      | deltoids record [trace-id] --capture /tmp/capture.tar

Input on stdin (JSON):
- tool: the tool that ran the command, such as "bash". Required.
- command: the command text. Optional; also the entry's reason by default.
- reason: why the command ran. Optional.
- origin: {"agent", "sessionId", "toolCallId"} of the caller. Optional.

Every changed file becomes part of one entry. A capture with no changes
records nothing. Recording the same capture twice adds it once.

Output:
- Success goes to stdout as JSON: ok, recorded, traceId, operationId, paths.
- Failure goes to stderr as JSON and exits non-zero.
"#;

#[derive(Debug, ClapArgs)]
#[command(after_help = OVERVIEW)]
pub struct Args {
    /// Existing trace to append to. Omit to start a new trace.
    pub trace_id: Option<String>,
    /// The archive Kao wrote on file descriptor 3.
    #[arg(long)]
    pub capture: PathBuf,
}

pub fn run(args: Args) -> ExitCode {
    match run_inner(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let response = ErrorResponse {
                ok: false,
                error,
                trace_id: None,
                message: None,
            };
            eprintln!(
                "{}",
                serde_json::to_string(&response).expect("error response should serialize")
            );
            ExitCode::from(1)
        }
    }
}

fn run_inner(args: Args) -> Result<(), String> {
    let mut input = String::new();
    io::stdin()
        .read_to_string(&mut input)
        .map_err(|err| format!("Failed to read stdin: {err}"))?;
    let request: RecordRequest =
        serde_json::from_str(&input).map_err(|err| format!("Invalid request JSON: {err}"))?;
    let archive = std::fs::read(&args.capture)
        .map_err(|err| format!("Failed to read {}: {err}", args.capture.display()))?;
    let store = TraceStore::from_env()?;
    let response = record_capture(&store, args.trace_id.as_deref(), &archive, request)?;
    println!(
        "{}",
        serde_json::to_string(&response).expect("record response should serialize")
    );
    Ok(())
}

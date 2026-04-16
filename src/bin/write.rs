use std::io::{self, IsTerminal, Read};
use std::process::ExitCode;

use clap::Parser;
use edit::{ErrorResponse, WriteRequest, execute_write_request};

const OVERVIEW: &str = r#"CLI for agents to rewrite files.

Input:
- summary: short description of the change. Required. Must not be empty.
- path: UTF-8 text file to write.
- content: full file contents to write.

Rules:
- Unknown JSON fields are rejected.
- If the path exists, it must be a file.
- Parent directories are created as needed.

Example:
printf '%s' '{
  "summary": "Rewrite config",
  "path": "config.json",
  "content": "{\n  \"version\": 2\n}\n"
}' | write

Output:
- Success goes to stdout as JSON.
- Failure goes to stderr as JSON and exits non-zero.
"#;

#[derive(Debug, Parser)]
#[command(
    name = "write",
    about = "CLI for agents to rewrite files.",
    after_help = OVERVIEW
)]
struct Cli {}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let response = ErrorResponse { ok: false, error };
            eprintln!(
                "{}",
                serde_json::to_string(&response).expect("error response should serialize")
            );
            ExitCode::from(1)
        }
    }
}

fn run() -> Result<(), String> {
    let _cli = Cli::parse();

    let mut stdin = io::stdin();
    if stdin.is_terminal() {
        print_overview();
        return Ok(());
    }

    let mut input = String::new();
    stdin
        .read_to_string(&mut input)
        .map_err(|err| format!("Failed to read stdin: {err}"))?;

    if should_show_overview(false, &input) {
        print_overview();
        return Ok(());
    }

    let request: WriteRequest =
        serde_json::from_str(&input).map_err(|err| format!("Invalid request JSON: {err}"))?;

    let response = execute_write_request(request)?;
    println!(
        "{}",
        serde_json::to_string(&response).expect("success response should serialize")
    );
    Ok(())
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

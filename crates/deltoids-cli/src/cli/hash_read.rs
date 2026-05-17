//! `deltoids hashread` — agent read tool that emits hashline anchors.

use std::io::{self, IsTerminal, Read};
use std::process::ExitCode;

use clap::Args as ClapArgs;

use crate::{ErrorResponse, HashReadRequest, execute_hash_read};

const OVERVIEW: &str = r#"CLI for agents to read files with hashline anchors.

Input:
- path: UTF-8 text file to read. Must exist and be a file.
- offset (optional): 1-indexed first line to return. Default: 1.
- limit (optional): maximum number of lines to return. Default: all.

Output:
- One line per content line, formatted as `LINEhh|TEXT` on stdout.
  `LINE` is the 1-indexed line number; `hh` is a 2-character content hash;
  `|` separates the anchor from the line content.
- Errors go to stderr as JSON and exit non-zero.

Notes:
- Anchors are stable across reads of unchanged content.
- Copy the anchor token (`LINEhh`, e.g. `42sr`) into `pos`/`end` fields of
  `deltoids hashedit` ops. NEVER include the `|TEXT` body in those fields.
- Trailing whitespace is ignored when hashing, so anchors survive
  line-ending or trailing-space-only changes.

Examples:
printf '%s' '{"path": "src/app.ts"}' | deltoids hashread
printf '%s' '{"path": "src/app.ts", "offset": 40, "limit": 20}' | deltoids hashread

deltoids hashread --path src/app.ts
deltoids hashread --path src/app.ts --offset 40 --limit 20
"#;

#[derive(Debug, Default, ClapArgs)]
#[command(after_help = OVERVIEW)]
pub struct Args {
    #[arg(long)]
    pub path: Option<String>,
    #[arg(long)]
    pub offset: Option<usize>,
    #[arg(long)]
    pub limit: Option<usize>,
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
        hash_read_request_from_shorthand(&args).map_err(simple_error)?
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

    let body = execute_hash_read(&request).map_err(simple_error)?;
    println!("{body}");
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
    args.path.is_some() || args.offset.is_some() || args.limit.is_some()
}

fn hash_read_request_from_shorthand(args: &Args) -> Result<HashReadRequest, String> {
    let path = args
        .path
        .clone()
        .ok_or_else(|| "--path is required".to_string())?;
    Ok(HashReadRequest {
        path,
        offset: args.offset,
        limit: args.limit,
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
        assert!(!super::should_show_overview(false, "{\"path\":\"x\"}"));
    }
}

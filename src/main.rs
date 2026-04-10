use std::io::{self, Read};
use std::process::ExitCode;

use clap::Parser;
use edit::{EditRequest, ErrorResponse, execute_request};

#[derive(Debug, Parser)]
#[command(name = "edit")]
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

    let mut input = String::new();
    io::stdin()
        .read_to_string(&mut input)
        .map_err(|err| format!("Failed to read stdin: {err}"))?;

    let request: EditRequest =
        serde_json::from_str(&input).map_err(|err| format!("Invalid request JSON: {err}"))?;

    let response = execute_request(request)?;
    println!(
        "{}",
        serde_json::to_string(&response).expect("success response should serialize")
    );
    Ok(())
}

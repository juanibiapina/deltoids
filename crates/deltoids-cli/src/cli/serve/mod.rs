//! `deltoids serve`: a read-only HTTP server that browses edit/write
//! traces across every project, plus a mobile-first web app for reviewing
//! them.
//!
//! The socket loop here is a thin shell: it accepts requests, spawns a
//! thread per request, and hands the method + URL to [`router::handle`],
//! which is pure and unit-tested. Live updates use client polling of
//! `/api/feed`, so no async runtime is needed.

mod assets;
mod router;

use std::process::ExitCode;

use clap::Args as ClapArgs;

use crate::TraceStore;

const OVERVIEW: &str = r#"Serve traces over HTTP for the mobile web reviewer.

Exposes a read-only API and a web app that browse edit/write traces
across every project. Open the printed URL on a phone on the same
network to swipe through edits.

Note: binding 0.0.0.0 (the default) exposes your traces, including the
source snippets in each diff, to other devices on the network. Use
--host 127.0.0.1 to restrict to this machine (e.g. behind a tunnel).
"#;

#[derive(Debug, ClapArgs)]
#[command(after_help = OVERVIEW)]
pub struct Args {
    /// Address to bind. Defaults to all interfaces for LAN access.
    #[arg(long, default_value = "0.0.0.0")]
    pub host: String,
    /// Port to listen on.
    #[arg(long, default_value_t = 8787)]
    pub port: u16,
}

pub fn run(args: Args) -> ExitCode {
    let store = match TraceStore::from_env() {
        Ok(store) => store,
        Err(err) => {
            eprintln!("deltoids serve: {err}");
            return ExitCode::FAILURE;
        }
    };

    let addr = format!("{}:{}", args.host, args.port);
    let server = match tiny_http::Server::http(addr.as_str()) {
        Ok(server) => server,
        Err(err) => {
            eprintln!("deltoids serve: failed to bind {addr}: {err}");
            return ExitCode::FAILURE;
        }
    };

    println!("deltoids serve listening on http://{addr}");
    for request in server.incoming_requests() {
        let store = store.clone();
        std::thread::spawn(move || respond(store, request));
    }

    ExitCode::SUCCESS
}

fn respond(store: TraceStore, request: tiny_http::Request) {
    let method = request.method().as_str().to_string();
    let target = request.url().to_string();
    let response = router::handle(&store, &method, &target);

    let content_type = tiny_http::Header::from_bytes(b"Content-Type", response.content_type)
        .expect("static content type is a valid header");
    // Always revalidate: the embedded assets change with each release, and
    // a phone caching a stale app.js would keep old behaviour (e.g. swipe
    // direction) after a rebuild.
    let no_store = tiny_http::Header::from_bytes(b"Cache-Control", b"no-store")
        .expect("static cache-control is a valid header");
    let http = tiny_http::Response::from_data(response.body)
        .with_status_code(response.status)
        .with_header(content_type)
        .with_header(no_store);
    let _ = request.respond(http);
}

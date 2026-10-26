//! Language servers: where a symbol is defined, who uses it, and what
//! the server says about it.
//!
//! [`Lsp`] is the whole interface. Ask a [`Query`] about a [`Symbol`],
//! get a [`Ticket`], and read its [`Reply`] once the server answers. Everything protocol-shaped stays inside: starting servers
//! (lazily, one per distinct command), the handshake, position encodings,
//! file URIs, keeping open documents in step with the text the caller
//! shows, the callers lookup (call hierarchy and references together,
//! calls through a trait included: [`callers`]), replies to the server's
//! own requests, progress, crashes, and shutdown.
//!
//! deltoids watches files for its servers: the workdir watcher feeds
//! [`Lsp::file_events`], and each server hears about the files it
//! registered for. A server that would watch on its own can take
//! minutes on a large tree.
//!
//! The seam is the transport ([`transport::Link`]): production spawns
//! the configured command, and tests connect a scripted fake server over
//! an in-memory connection.

use std::cell::RefCell;
use std::collections::HashMap;
use std::io;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use deltoids::Language;

mod callers;
mod config;
mod position;
mod server;
mod transport;
mod watched_files;

#[cfg(test)]
pub(crate) mod fake;
#[cfg(test)]
mod tests;

use config::{ServerSpec, ServerTable};
use server::{Ask, Hit, Payload, Server};
use transport::Link;
pub(crate) use watched_files::FileEvents;

/// A symbol to ask about: one identifier on one line of a file whose
/// text matches the file on disk.
pub(crate) struct Symbol<'a> {
    pub(crate) language: Language,
    /// Workdir-relative, as the sidebar shows it.
    pub(crate) path: &'a str,
    /// The whole file.
    pub(crate) text: &'a str,
    /// 1-based.
    pub(crate) line: usize,
    /// Byte range of the identifier within the line.
    pub(crate) columns: Range<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Query {
    Definition,
    Callers,
    /// The server's info about the symbol: its type, signature, and docs.
    Hover,
}

/// A query's answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Reply {
    /// For [`Query::Definition`] and [`Query::Callers`]: never empty.
    Places(Vec<Found>),
    /// For [`Query::Hover`]: Markdown, never empty.
    Info(String),
}

/// One answer location.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Found {
    /// Workdir-relative; absolute when outside the workdir.
    pub(crate) path: String,
    /// 1-based.
    pub(crate) line: usize,
    /// The calling function, for [`Query::Callers`].
    pub(crate) caller: Option<String>,
}

/// A pending query. Read it with [`Lsp::answer`] or drop it with
/// [`Lsp::cancel`].
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Ticket(u64);

/// What a language's server is doing, for a status mark.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum State {
    /// No server: none configured, its program is missing, or it has not
    /// been started.
    Off,
    /// Starting, or reporting work such as indexing, with its percentage
    /// when the server gives one.
    Loading(Option<u8>),
    /// Running and idle: lookups answer now.
    Ready,
    /// Stopped or crashed, or failed to start; the next lookup retries.
    Failed,
}

/// What a running server is busy with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// Started; the handshake has not finished.
    Starting,
    /// Reporting work, such as indexing the project.
    Working { percent: Option<u8> },
}

/// Connects a server: production spawns its process.
type Launch = Box<dyn Fn(&ServerSpec, &Path) -> io::Result<Link>>;

/// The language servers of one repository. Clones share the servers;
/// they shut down when the last clone drops.
#[derive(Clone)]
pub(crate) struct Lsp(Rc<RefCell<Inner>>);

struct Inner {
    root: PathBuf,
    /// `root` and its canonical form, to make server paths relative.
    roots: Vec<PathBuf>,
    table: ServerTable,
    launch: Launch,
    servers: HashMap<ServerSpec, Server>,
    /// Servers that did not start, and whether their program is missing.
    failures: HashMap<ServerSpec, bool>,
    next_ticket: u64,
    answers: HashMap<u64, Result<Reply, String>>,
    /// Changes to the files the servers watch, fed by the workdir watcher.
    files: FileEvents,
}

impl Lsp {
    /// The servers for the repository at `root`, configured from
    /// `config.toml` and started as processes.
    pub(crate) fn discover(root: &Path) -> Self {
        Self::with_launcher(root, ServerTable::load(), Box::new(transport::spawn))
    }

    fn with_launcher(root: &Path, table: ServerTable, launch: Launch) -> Self {
        let mut roots = vec![root.to_path_buf()];
        if let Ok(canonical) = root.canonicalize()
            && canonical != root
        {
            roots.push(canonical);
        }
        Lsp(Rc::new(RefCell::new(Inner {
            root: root.to_path_buf(),
            files: FileEvents::new(),
            roots,
            table,
            launch,
            servers: HashMap::new(),
            failures: HashMap::new(),
            next_ticket: 1,
            answers: HashMap::new(),
        })))
    }

    /// Where the workdir watcher reports changes on disk. Each server
    /// hears about the files it registered for at the next pump; every
    /// method here pumps first, and [`Lsp::poll`] pumps alone.
    pub(crate) fn file_events(&self) -> FileEvents {
        self.0.borrow().files.clone()
    }

    /// Handle what the servers sent and pass on changes on disk. Call it
    /// regularly while nothing else here is called, so the servers keep
    /// up with edits made elsewhere.
    pub(crate) fn poll(&self) {
        self.0.borrow_mut().pump();
    }

    /// Whether `language` has a configured server.
    pub(crate) fn serves(&self, language: Language) -> bool {
        self.0.borrow().table.get(language).is_some()
    }

    /// Start `language`'s server in the background, so the first query
    /// answers sooner. Does nothing when it runs already, has no server,
    /// or cannot start (the next [`Lsp::ask`] reports why).
    pub(crate) fn warm(&self, language: Language) {
        let mut inner = self.0.borrow_mut();
        inner.pump();
        let _ = inner.server(language);
    }

    /// Ask `query` about `symbol`. Fails at once when `symbol`'s language
    /// has no server or the server cannot start; every later failure
    /// arrives through [`Lsp::answer`].
    pub(crate) fn ask(&self, query: Query, symbol: Symbol<'_>) -> Result<Ticket, String> {
        let mut inner = self.0.borrow_mut();
        inner.pump();
        let uri = position::uri(&inner.root.join(symbol.path))
            .ok_or_else(|| format!("{} is not a file path", symbol.path))?;
        let ticket = inner.next_ticket;
        inner.next_ticket += 1;
        let server = inner.server(symbol.language)?;
        server.open(&uri, config::language_id(symbol.language), symbol.text);
        let index = symbol.line.saturating_sub(1);
        server.ask(Ask {
            ticket,
            query,
            uri,
            line: index,
            line_text: symbol
                .text
                .lines()
                .nth(index)
                .unwrap_or_default()
                .to_string(),
            byte: symbol.columns.start,
        });
        Ok(Ticket(ticket))
    }

    /// `ticket`'s answer, once the server sent it. `None` while pending.
    pub(crate) fn answer(&self, ticket: &Ticket) -> Option<Result<Reply, String>> {
        let mut inner = self.0.borrow_mut();
        inner.pump();
        inner.answers.remove(&ticket.0)
    }

    /// Drop `ticket`: its answer never arrives.
    pub(crate) fn cancel(&self, ticket: Ticket) {
        let mut inner = self.0.borrow_mut();
        for server in inner.servers.values_mut() {
            server.cancel(ticket.0);
        }
        inner.answers.remove(&ticket.0);
    }

    /// What `language`'s server is doing.
    pub(crate) fn state(&self, language: Language) -> State {
        let mut inner = self.0.borrow_mut();
        inner.pump();
        let Some(spec) = inner.table.get(language) else {
            return State::Off;
        };
        match inner.servers.get(spec) {
            Some(server) if server.stopped() => State::Failed,
            Some(server) => match server.phase() {
                Some(Phase::Starting) => State::Loading(None),
                Some(Phase::Working { percent }) => State::Loading(percent),
                None => State::Ready,
            },
            None => match inner.failures.get(spec) {
                Some(false) => State::Failed,
                _ => State::Off,
            },
        }
    }

    /// Bring open documents in step with `files` (workdir-relative path,
    /// text), as the caller now shows them.
    pub(crate) fn sync<'a>(&self, files: impl IntoIterator<Item = (&'a str, &'a str)>) {
        let mut inner = self.0.borrow_mut();
        let files: Vec<(String, &str)> = files
            .into_iter()
            .filter_map(|(path, text)| Some((position::uri(&inner.root.join(path))?, text)))
            .collect();
        for server in inner.servers.values_mut() {
            for (uri, text) in &files {
                server.sync(uri, text);
            }
        }
    }
}

/// Why `spec`'s server for `language` did not start.
fn start_error(language: Language, spec: &ServerSpec, error: &io::Error) -> String {
    if error.kind() == io::ErrorKind::NotFound {
        format!(
            "No language server for {} ({} not found)",
            language.name(),
            spec.name()
        )
    } else {
        format!("{} failed to start: {error}", spec.name())
    }
}

impl Inner {
    /// Collect every server's answers and pass on changes on disk.
    fn pump(&mut self) {
        let mut answered = Vec::new();
        if self.files.take_lost() {
            self.restart_watching(&mut answered);
        }
        let mut watched_changed = false;
        for server in self.servers.values_mut() {
            answered.extend(server.pump());
            watched_changed |= server.take_watched_changed();
        }
        if watched_changed {
            self.refilter();
        }
        let changes = self.files.take();
        if !changes.is_empty() {
            for server in self.servers.values_mut() {
                server.files_changed(&changes);
            }
        }
        for (ticket, answer) in answered {
            let reply = answer.map(|payload| match payload {
                Payload::Hits(hits) => {
                    Reply::Places(hits.into_iter().map(|hit| self.found(hit)).collect())
                }
                Payload::Info(text) => Reply::Info(text),
            });
            self.answers.insert(ticket, reply);
        }
    }

    /// `language`'s running server, started when it is not running or
    /// stopped.
    fn server(&mut self, language: Language) -> Result<&mut Server, String> {
        let spec = self
            .table
            .get(language)
            .cloned()
            .ok_or_else(|| format!("No language server for {}", language.name()))?;
        if self.servers.get(&spec).is_none_or(Server::stopped) {
            self.servers.remove(&spec);
            self.start(&spec)
                .map_err(|error| start_error(language, &spec, &error))?;
        }
        Ok(self.servers.get_mut(&spec).expect("inserted above"))
    }

    fn start(&mut self, spec: &ServerSpec) -> io::Result<()> {
        let link = (self.launch)(spec, &self.root).inspect_err(|error| {
            let missing = error.kind() == io::ErrorKind::NotFound;
            self.failures.insert(spec.clone(), missing);
        })?;
        self.failures.remove(spec);
        let server = Server::start(spec, link, &self.root);
        self.servers.insert(spec.clone(), server);
        Ok(())
    }

    /// Start every server that watches files again, so it reads the
    /// disk afresh: changes went missing and no message says which.
    /// Their queries in flight fail.
    fn restart_watching(&mut self, answered: &mut Vec<(u64, server::Answer)>) {
        let specs: Vec<ServerSpec> = self
            .servers
            .iter()
            .filter(|(_, server)| server.watched().any())
            .map(|(spec, _)| spec.clone())
            .collect();
        for spec in specs {
            if let Some(mut server) = self.servers.remove(&spec) {
                answered.extend(server.abandon(&format!(
                    "{} restarted to catch up with changes on disk; ask again",
                    spec.name()
                )));
            }
            let _ = self.start(&spec);
        }
    }

    /// Keep the changes some server watches, as the servers watch now.
    /// Only a change to what a server watches calls for this; a server
    /// that goes away leaves patterns that reach no one.
    fn refilter(&mut self) {
        let sets = self
            .servers
            .values()
            .map(Server::watched)
            .flat_map(|watched| watched.sets())
            .collect();
        self.files.watch(sets);
    }

    fn found(&self, hit: Hit) -> Found {
        let path = self
            .roots
            .iter()
            .find_map(|root| hit.path.strip_prefix(root).ok())
            .map_or_else(
                || hit.path.to_string_lossy().into_owned(),
                |relative| {
                    relative
                        .components()
                        .map(|part| part.as_os_str().to_string_lossy())
                        .collect::<Vec<_>>()
                        .join("/")
                },
            );
        Found {
            path,
            line: hit.line,
            caller: hit.caller,
        }
    }
}

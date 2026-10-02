//! Catalog of histories for one project. Notifications name histories to
//! refresh; reconciliation checks metadata before reading any unchanged file.

use std::collections::{HashMap, HashSet, hash_map::DefaultHasher};
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use super::model::LoadedTrace;
use crate::trace_store::{parse_history_entry, trace_summary_from, validate_trace_id};
use crate::{HistoryEntry, TraceStore};

const MAX_PENDING: usize = 4096;

#[derive(Clone, PartialEq, Eq)]
struct Stamp {
    length: u64,
    modified: SystemTime,
    #[cfg(unix)]
    identity: (u64, u64),
}

impl Stamp {
    fn read(path: &Path) -> Result<Option<Self>, String> {
        let metadata = match fs::metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(format!("Failed to inspect {}: {error}", path.display())),
        };
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;
        Ok(Some(Self {
            length: metadata.len(),
            modified: metadata.modified().map_err(|error| error.to_string())?,
            #[cfg(unix)]
            identity: (metadata.dev(), metadata.ino()),
        }))
    }
}

struct History {
    stamp: Stamp,
    committed: usize,
    prefix: u64,
    lines: usize,
    entries: Vec<HistoryEntry>,
    fingerprints: Vec<u64>,
}

#[cfg(test)]
mod tests;

#[derive(Debug)]
pub(super) struct Refresh {
    pub traces: Vec<LoadedTrace>,
    /// Earlier entries still identify the same recorded source.
    pub retained: Vec<(String, usize)>,
}

pub(super) struct TraceReader {
    store: TraceStore,
    cwd: String,
    histories: HashMap<String, History>,
    pending: HashSet<String>,
    reconcile: bool,
}

impl TraceReader {
    pub fn new(store: TraceStore, cwd: String) -> Self {
        Self {
            store,
            cwd,
            histories: HashMap::new(),
            pending: HashSet::new(),
            reconcile: true,
        }
    }

    pub fn notify(&mut self, paths: &[PathBuf], rescan: bool) -> bool {
        self.reconcile |= rescan;
        for path in paths {
            let Ok(relative) = path.strip_prefix(self.store.root()) else {
                self.reconcile = true;
                continue;
            };
            let mut components = relative.iter();
            let Some(id) = components.next().and_then(|id| id.to_str()) else {
                self.reconcile = true;
                continue;
            };
            if validate_trace_id(id).is_err() {
                continue;
            }
            let child = components.next();
            if child.is_some_and(|name| name != "entries.jsonl") {
                continue;
            }
            if self.pending.len() >= MAX_PENDING {
                self.reconcile = true;
                self.pending.clear();
                break;
            }
            self.pending.insert(id.to_owned());
        }
        self.needs_retry()
    }

    pub fn needs_retry(&self) -> bool {
        self.reconcile || !self.pending.is_empty()
    }

    pub fn refresh(&mut self) -> Result<Option<Refresh>, String> {
        let mut ids = self.pending.clone();
        if self.reconcile {
            ids.extend(self.changed_histories()?);
        }
        let mut updates = Vec::new();
        for id in &ids {
            let path = self.store.trace_directory(id).join("entries.jsonl");
            let stamp = Stamp::read(&path)?;
            let update = match stamp {
                Some(stamp) => Some(self.read_history(&path, stamp, self.histories.get(id))?),
                None => None,
            };
            updates.push((id.clone(), update));
        }
        // Publish only after every changed history was read successfully.
        self.reconcile = false;
        self.pending.clear();
        let mut visible_changed = false;
        let mut prefixes = HashMap::new();
        for (id, update) in updates {
            let old = self
                .histories
                .get(&id)
                .map(|history| history.fingerprints.as_slice())
                .unwrap_or(&[]);
            let new = update
                .as_ref()
                .map(|(history, _)| history.fingerprints.as_slice())
                .unwrap_or(&[]);
            visible_changed |= old != new;
            prefixes.insert(
                id.clone(),
                old.iter()
                    .zip(new)
                    .take_while(|(left, right)| left == right)
                    .count(),
            );
            let Some((history, partial)) = update else {
                self.histories.remove(&id);
                continue;
            };
            if partial {
                self.pending.insert(id.clone());
            }
            self.histories.insert(id, history);
        }
        if !visible_changed {
            return Ok(None);
        }
        let mut traces = Vec::new();
        let mut retained = Vec::new();
        for (id, history) in &self.histories {
            let refs: Vec<_> = history.entries.iter().collect();
            if let Some(trace) = trace_summary_from(id, &refs) {
                traces.push(LoadedTrace {
                    trace,
                    entries: history.entries.clone(),
                });
                retained.push((
                    id.clone(),
                    prefixes.get(id).copied().unwrap_or(history.entries.len()),
                ));
            }
        }
        traces.sort_by(|left, right| {
            right
                .trace
                .last_timestamp
                .cmp(&left.trace.last_timestamp)
                .then_with(|| left.trace.trace_id.cmp(&right.trace.trace_id))
        });
        Ok(Some(Refresh { traces, retained }))
    }

    fn changed_histories(&self) -> Result<HashSet<String>, String> {
        let mut changed = HashSet::new();
        let mut present = HashSet::new();
        let directories = match fs::read_dir(self.store.root()) {
            Ok(directories) => directories,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(self.histories.keys().cloned().collect());
            }
            Err(error) => return Err(format!("Failed to list traces: {error}")),
        };
        for directory in directories {
            let directory = directory.map_err(|error| error.to_string())?;
            let id = directory.file_name().to_string_lossy().into_owned();
            if validate_trace_id(&id).is_err()
                || !directory
                    .file_type()
                    .map_err(|error| error.to_string())?
                    .is_dir()
            {
                continue;
            }
            let Some(stamp) = Stamp::read(&directory.path().join("entries.jsonl"))? else {
                continue;
            };
            if self
                .histories
                .get(&id)
                .is_none_or(|history| history.stamp != stamp)
            {
                changed.insert(id.clone());
            }
            present.insert(id);
        }
        changed.extend(
            self.histories
                .keys()
                .filter(|id| !present.contains(*id))
                .cloned(),
        );
        Ok(changed)
    }

    fn read_history(
        &self,
        path: &Path,
        stamp: Stamp,
        old: Option<&History>,
    ) -> Result<(History, bool), String> {
        let bytes = fs::read(path)
            .map_err(|error| format!("Failed to read {}: {error}", path.display()))?;
        if Stamp::read(path)?.as_ref() != Some(&stamp) {
            return Err(format!("History changed while reading {}", path.display()));
        }
        // A size increase alone cannot prove an append. Validate the consumed
        // prefix of this changed history before retaining any parsed entries.
        let append = old.is_some_and(|old| {
            bytes.len() >= old.committed && digest(&bytes[..old.committed]) == old.prefix
        });
        let mut history = History {
            stamp,
            committed: if append { old.unwrap().committed } else { 0 },
            prefix: 0,
            lines: if append { old.unwrap().lines } else { 0 },
            entries: if append {
                old.unwrap().entries.clone()
            } else {
                Vec::new()
            },
            fingerprints: if append {
                old.unwrap().fingerprints.clone()
            } else {
                Vec::new()
            },
        };
        let mut partial = false;
        for bytes_line in bytes[history.committed..].split_inclusive(|byte| *byte == b'\n') {
            let terminated = bytes_line.ends_with(b"\n");
            let line = match std::str::from_utf8(bytes_line) {
                Ok(line) => line,
                Err(error) if !terminated && error.error_len().is_none() => {
                    partial = true;
                    break;
                }
                Err(error) => {
                    return Err(format!(
                        "Invalid history text in {}: {error}",
                        path.display()
                    ));
                }
            };
            if line.trim().is_empty() {
                history.lines += usize::from(terminated);
                history.committed += bytes_line.len();
                continue;
            }
            let entry = match parse_history_entry(line) {
                Ok(entry) => entry,
                Err(error) if !terminated && error.is_eof() => {
                    partial = true;
                    break;
                }
                Err(error) => {
                    return Err(format!(
                        "Failed to parse history entry {} in {}: {error}",
                        history.lines + 1,
                        path.display()
                    ));
                }
            };
            if entry.cwd == self.cwd {
                history.fingerprints.push(digest(line.trim().as_bytes()));
                history.entries.push(entry);
            }
            history.lines += usize::from(terminated);
            history.committed += bytes_line.len();
        }
        history.prefix = digest(&bytes[..history.committed]);
        Ok((history, partial))
    }
}

fn digest(bytes: &[u8]) -> u64 {
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}

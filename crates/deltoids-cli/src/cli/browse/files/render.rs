//! Background file rendering and retained blocks. The UI only submits demand
//! and adopts completed rows; Git, comments, and terminal state stay on the UI.
//!
//! Each staging column's pane owns one cache and always renders the same
//! body for a file index ([`Model::view`] for its column) until a reload
//! clears it: a file only gains or loses its split bodies through a rebuilt
//! model.

use std::collections::{HashMap, HashSet, VecDeque};
use std::ops::Range;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, mpsc};

use deltoids::Theme;
use deltoids::parse::FileDiff;

use super::diff_pane::{CacheEpoch, render_file_block};
use super::model::{Column, FileBody, Model};
use crate::cli::browse::comments::Numbering;
use crate::cli::browse::diff_cursor::DiffRow;

const RETAINED_BYTES: usize = 64 * 1024 * 1024;
const MAX_PENDING_FILES: usize = 8;

struct Block {
    rows: Vec<DiffRow>,
    bytes: usize,
    touched: u64,
    complete: bool,
}

struct Job {
    key: usize,
    token: u64,
    wanted: Arc<AtomicBool>,
    epoch: CacheEpoch,
    file: FileDiff,
    body: FileBody,
    numbering: Numbering,
    theme: Theme,
}

struct Completion {
    key: usize,
    token: u64,
    rows: Vec<DiffRow>,
    complete: bool,
}

#[derive(Default)]
struct Queue {
    jobs: VecDeque<Job>,
    stopped: bool,
}

struct Worker {
    queue: Arc<(Mutex<Queue>, Condvar)>,
    token: Arc<AtomicU64>,
    results: mpsc::Receiver<Completion>,
}

impl Worker {
    fn start() -> Self {
        let queue = Arc::new((Mutex::new(Queue::default()), Condvar::new()));
        let token = Arc::new(AtomicU64::new(0));
        let (sender, results) = mpsc::sync_channel(2);
        let work_queue = Arc::clone(&queue);
        let work_token = Arc::clone(&token);
        std::thread::spawn(move || run_worker(work_queue, work_token, sender));
        Self {
            queue,
            token,
            results,
        }
    }

    fn replace(&self, jobs: VecDeque<Job>) {
        let (mutex, wake) = &*self.queue;
        mutex.lock().expect("render queue").jobs = jobs;
        wake.notify_one();
    }

    fn enqueue(&self, jobs: VecDeque<Job>) {
        let (mutex, wake) = &*self.queue;
        mutex.lock().expect("render queue").jobs.extend(jobs);
        wake.notify_one();
    }

    fn prioritize(&self, demand: &[usize]) {
        let ranks: HashMap<_, _> = demand
            .iter()
            .enumerate()
            .map(|(rank, key)| (*key, rank))
            .collect();
        let (mutex, _) = &*self.queue;
        let mut queue = mutex.lock().expect("render queue");
        queue.jobs.retain(|job| job.wanted.load(Ordering::Relaxed));
        queue
            .jobs
            .make_contiguous()
            .sort_by_key(|job| ranks.get(&job.key).copied().unwrap_or(usize::MAX));
    }

    fn cancel(&self) {
        self.token.fetch_add(1, Ordering::Relaxed);
        self.replace(VecDeque::new());
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.token.fetch_add(1, Ordering::Relaxed);
        let (mutex, wake) = &*self.queue;
        let mut queue = mutex.lock().expect("render queue");
        queue.stopped = true;
        queue.jobs.clear();
        wake.notify_one();
        // Dropping the receiver releases a worker blocked on its bounded sender.
        // Never join: one pathological syntax line must not delay quitting.
    }
}

fn run_worker(
    queue: Arc<(Mutex<Queue>, Condvar)>,
    token: Arc<AtomicU64>,
    sender: mpsc::SyncSender<Completion>,
) {
    loop {
        let job = {
            let (mutex, wake) = &*queue;
            let mut queue = mutex.lock().expect("render queue");
            while queue.jobs.is_empty() && !queue.stopped {
                queue = wake.wait(queue).expect("render queue");
            }
            if queue.stopped {
                return;
            }
            queue.jobs.pop_front().expect("queued job")
        };
        let keep_going =
            || token.load(Ordering::Relaxed) == job.token && job.wanted.load(Ordering::Relaxed);
        let mut publish = |rows| {
            if keep_going()
                && sender
                    .send(Completion {
                        key: job.key,
                        token: job.token,
                        rows,
                        complete: false,
                    })
                    .is_err()
            {
                job.wanted.store(false, Ordering::Relaxed);
            }
        };
        if let Some(rows) = render_file_block(
            &job.file,
            &job.body,
            job.numbering,
            job.epoch.width,
            job.epoch.layout,
            &job.theme,
            &keep_going,
            &mut publish,
        ) && keep_going()
            && sender
                .send(Completion {
                    key: job.key,
                    token: job.token,
                    rows,
                    complete: true,
                })
                .is_err()
        {
            return;
        }
    }
}

#[derive(Default)]
pub(super) struct DiffCache {
    epoch: CacheEpoch,
    rows: HashMap<usize, Block>,
    pub(super) revision: u64,
    worker: Option<Worker>,
    demand: Vec<usize>,
    visible: HashSet<usize>,
    pending: HashMap<usize, Arc<AtomicBool>>,
    scheduled: HashSet<usize>,
    bytes: usize,
    clock: u64,
    budget: Budget,
}

struct Budget(usize);

impl Default for Budget {
    fn default() -> Self {
        Self(RETAINED_BYTES)
    }
}

impl DiffCache {
    pub(super) fn get(&self, epoch: CacheEpoch, key: usize) -> Option<&[DiffRow]> {
        if self.epoch != epoch {
            return None;
        }
        self.rows.get(&key).map(|block| block.rows.as_slice())
    }

    pub(super) fn complete(&self, epoch: CacheEpoch, key: usize) -> bool {
        self.epoch == epoch && self.rows.get(&key).is_some_and(|block| block.complete)
    }

    #[cfg(test)]
    pub(super) fn contains(&self, epoch: CacheEpoch, key: usize) -> bool {
        self.get(epoch, key).is_some()
    }

    pub(super) fn insert(&mut self, epoch: CacheEpoch, key: usize, rows: Vec<DiffRow>) {
        if self.epoch != epoch {
            self.clear();
            self.epoch = epoch;
        }
        let bytes = block_bytes(&rows, rows.capacity());
        if let Some(old) = self.rows.remove(&key) {
            self.bytes -= old.bytes;
        }
        self.clock = self.clock.wrapping_add(1);
        self.rows.insert(
            key,
            Block {
                rows,
                bytes,
                touched: self.clock,
                complete: true,
            },
        );
        self.bytes += bytes;
        self.evict();
        self.revision = self.revision.wrapping_add(1);
    }

    pub(super) fn clear(&mut self) {
        self.pause();
        self.rows.clear();
        self.bytes = 0;
        self.revision = self.revision.wrapping_add(1);
    }

    /// Cancel demand on focus loss or mode deactivation, retaining valid blocks.
    pub(super) fn pause(&mut self) {
        if self.pending.is_empty() && self.demand.is_empty() {
            return;
        }
        if let Some(worker) = &self.worker {
            worker.cancel();
        }
        self.pending.clear();
        self.scheduled.clear();
        self.demand.clear();
    }

    pub(super) fn pending(&self) -> bool {
        !self.pending.is_empty()
    }

    pub(super) fn visible_pending(&self) -> bool {
        self.visible
            .iter()
            .any(|key| !self.complete(self.epoch, *key))
    }

    /// Adopt only completions belonging to the current demand token. Reloads,
    /// epoch changes, and focus loss all cancel that token before changing state.
    pub(super) fn collect(&mut self) -> bool {
        let mut visible_changed = false;
        while let Some(worker) = &self.worker {
            match worker.results.try_recv() {
                Ok(result) if result.token == worker.token.load(Ordering::Relaxed) => {
                    visible_changed |= self.adopt(result);
                }
                Ok(_) => {}
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    visible_changed |= self.visible.iter().any(|key| !self.rows.contains_key(key));
                    self.pending.clear();
                    self.scheduled.clear();
                    self.demand.clear();
                    self.worker = None;
                    break;
                }
            }
        }
        visible_changed
    }

    fn adopt(&mut self, result: Completion) -> bool {
        if !result.complete && self.complete(self.epoch, result.key) {
            return false;
        }
        if result.complete
            && let Some(wanted) = self.pending.remove(&result.key)
        {
            wanted.store(false, Ordering::Relaxed);
        }
        let visible = self.visible.contains(&result.key);
        let revision = self.revision;
        self.insert(self.epoch, result.key, result.rows);
        if let Some(block) = self.rows.get_mut(&result.key) {
            block.complete = result.complete;
        }
        if !visible {
            self.revision = revision;
        }
        visible
    }

    /// Demand `order[range]` at `epoch`, rendering each file's body for
    /// `column`, plus near neighbours of a single selected file.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn request(
        &mut self,
        epoch: CacheEpoch,
        model: &Model,
        column: Column,
        order: &[usize],
        range: Option<Range<usize>>,
        priority: Option<usize>,
        theme: &Theme,
    ) {
        if epoch != self.epoch {
            self.clear();
            self.epoch = epoch;
        }
        let range = range.unwrap_or(0..0);
        let visible: Vec<usize> = order[range.clone()].to_vec();
        let mut demand = visible.clone();
        if let Some(position) = priority.and_then(|key| demand.iter().position(|item| *item == key))
        {
            demand.rotate_left(position);
        }
        if range.len() == 1 {
            let forward = self.demand.first().is_none_or(|old| {
                order.iter().position(|key| key == old).unwrap_or(0) <= range.start
            });
            demand.extend(
                neighbors(order, range.start, forward)
                    .filter(|key| body_lines(model.view(*key, column).body) <= 2000),
            );
        }
        self.visible = visible.into_iter().collect();
        self.clock = self.clock.wrapping_add(1);
        for key in &demand {
            if let Some(block) = self.rows.get_mut(key) {
                block.touched = self.clock;
            }
        }
        let missing_first = demand
            .first()
            .is_some_and(|key| self.get(self.epoch, *key).is_none());
        let retain_count = if range.len() > 1 || missing_first {
            1
        } else {
            demand.len()
        };
        if demand != self.demand {
            self.pending.retain(|key, wanted| {
                let keep = demand[..retain_count].contains(key);
                wanted.store(keep, Ordering::Relaxed);
                keep
            });
            self.scheduled = self.pending.keys().copied().collect();
            if let Some(worker) = &self.worker {
                worker.prioritize(&demand);
            }
            self.demand = demand;
        }
        self.schedule(model, column, theme);
        self.evict();
    }

    fn schedule(&mut self, model: &Model, column: Column, theme: &Theme) {
        let missing: Vec<usize> = self
            .demand
            .iter()
            .copied()
            .filter(|key| !self.complete(self.epoch, *key) && !self.scheduled.contains(key))
            .take(MAX_PENDING_FILES.saturating_sub(self.pending.len()))
            .collect();
        if missing.is_empty() {
            return;
        }
        let worker = self.worker.get_or_insert_with(Worker::start);
        let token = worker.token.load(Ordering::Relaxed);
        let mut jobs = VecDeque::new();
        for key in missing {
            let wanted = Arc::new(AtomicBool::new(true));
            self.pending.insert(key, Arc::clone(&wanted));
            self.scheduled.insert(key);
            let view = model.view(key, column);
            jobs.push_back(Job {
                key,
                token,
                wanted,
                epoch: self.epoch,
                file: render_metadata(view.file),
                body: view.body.clone(),
                numbering: view.numbering,
                theme: theme.clone(),
            });
        }
        worker.enqueue(jobs);
        worker.prioritize(&self.demand);
    }

    fn evict(&mut self) {
        while self.bytes > self.budget.0 {
            let victim = self
                .rows
                .iter()
                .filter(|(key, _)| !self.visible.contains(key))
                .min_by_key(|(_, block)| block.touched)
                .map(|(key, _)| *key);
            let Some(key) = victim else {
                break;
            };
            self.bytes -= self.rows.remove(&key).expect("retained block").bytes;
        }
    }

    #[cfg(test)]
    pub(super) fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

fn neighbors(order: &[usize], position: usize, forward: bool) -> impl Iterator<Item = usize> + '_ {
    (1..=2).flat_map(move |distance| {
        let next = position
            .checked_add(distance)
            .and_then(|pos| order.get(pos));
        let previous = position
            .checked_sub(distance)
            .and_then(|pos| order.get(pos));
        let pair = if forward {
            [next, previous]
        } else {
            [previous, next]
        };
        pair.into_iter().flatten().copied()
    })
}

fn body_lines(body: &FileBody) -> usize {
    match body {
        FileBody::Diff(diff) => diff.hunks().iter().map(|hunk| hunk.lines.len()).sum(),
        _ => 0,
    }
}

/// Patch hunks and resolved source strings are not render inputs. The computed
/// body already owns the scope-expanded hunks, shared with jobs through Arc.
fn render_metadata(file: &FileDiff) -> FileDiff {
    FileDiff {
        preamble: file.preamble.clone(),
        old_path: file.old_path.clone(),
        new_path: file.new_path.clone(),
        rename_from: file.rename_from.clone(),
        old_hash: file.old_hash.clone(),
        new_hash: file.new_hash.clone(),
        old_mode: file.old_mode.clone(),
        new_mode: file.new_mode.clone(),
        hunks: Vec::new(),
    }
}

fn block_bytes(rows: &[DiffRow], capacity: usize) -> usize {
    capacity * std::mem::size_of::<DiffRow>()
        + rows
            .iter()
            .map(|row| {
                row.line.spans.capacity() * std::mem::size_of::<ratatui::text::Span<'static>>()
                    + row
                        .line
                        .spans
                        .iter()
                        .map(|span| match &span.content {
                            std::borrow::Cow::Owned(text) => text.capacity(),
                            std::borrow::Cow::Borrowed(_) => 0,
                        })
                        .sum::<usize>()
                    + row
                        .anchor
                        .as_ref()
                        .map_or(0, |anchor| anchor.path.capacity())
                    + row.place.as_ref().map_or(0, |place| place.file.capacity())
            })
            .sum::<usize>()
}

#[cfg(test)]
mod tests;

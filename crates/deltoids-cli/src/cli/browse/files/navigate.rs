//! Go to a definition, list a function's callers, or show what the
//! server says about a symbol, from the diff, through language servers.
//!
//! This slice owns the symbol under the cursor (underlined, moved with
//! `h`/`l`, the arrows, or a click), the symbol under the pointer (for hover), the
//! pending lookup and its footer spinner, the callers list, jumps and the
//! way back (`Ctrl+o`), and visited files: unchanged files a jump opens
//! whole.
//!
//! One rule decides which lines can be asked about and landed on: the
//! body drawing the line must show the same text as the file on disk,
//! because that is the text the language server reads. It holds for the
//! working tree's unstaged view, for staged lines with nothing unstaged on
//! top, and for commits and branches whose files have not changed since.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Instant, SystemTime};

use crossterm::event::{KeyCode, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};

use deltoids::render_tui::{
    byte_at, line_cells, pane_block_with_title_line, pane_border_color, rgb_to_color,
};
use deltoids::{Diff, Language, LineKind, LineMap, Symbols, Theme};

use super::hover::{Hover, Spot};
use super::model::{Column, FileBody, Model};
use super::{FilesMode, Focus};
use crate::cli::browse::comments::{CommentAnchor, LineSide};
use crate::cli::browse::diff_scrollbar;
use crate::cli::browse::syntax_badge::SyntaxBadge;
use crate::cli::browse::text::display_width;
use crate::lsp::{FileEvents, Found, Lsp, Query, Reply, State, Symbol, Ticket};
use crate::sidebar::{IconMode, filter_icon};

/// Spinner frames, one per 100 ms.
const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
/// Jumps remembered for `Ctrl+o`.
const BACK_LIMIT: usize = 50;

/// Navigation state of one Files view.
pub(super) struct Navigation {
    lsp: Option<Lsp>,
    /// The repository's working directory, where files are read from.
    root: Option<PathBuf>,
    disk: HashMap<String, Disk>,
    symbols: HashMap<String, (u64, Arc<Symbols>)>,
    /// Per file, how its shown version lines up with the disk, keyed by
    /// both texts' fingerprints.
    maps: HashMap<String, ((u64, u64), Arc<LineMap>)>,
    /// The symbol picked on a line: which line, and its index there.
    picked: Option<(CommentAnchor, usize)>,
    pending: Option<Pending>,
    list: Option<List>,
    hover: Hover,
    back: Vec<Origin>,
    /// Paths opened as visited files, in the order they were opened.
    visited: Vec<String>,
    warmed: HashSet<Language>,
    /// The languages of the files the pane showed at the last draw.
    languages: Vec<Language>,
    started: Instant,
    frame: usize,
    /// Whether the last tick was busy, so the tick that ends the work
    /// draws the spinner away.
    busy: bool,
}

/// A file as last read from disk.
struct Disk {
    stamp: (u64, Option<SystemTime>),
    text: Arc<str>,
}

struct Pending {
    ticket: Ticket,
    query: Query,
    name: String,
    /// Where the symbol asked about is: an answer there is no place to go.
    path: String,
    line: usize,
}

/// The list of several answers.
struct List {
    title: String,
    entries: Vec<Entry>,
    selected: usize,
    /// Where the last draw put the popup and each entry's row.
    area: Rect,
    rows: Vec<(u16, usize)>,
}

struct Entry {
    found: Found,
    place: Place,
    code: String,
}

/// Where an answer lands.
enum Place {
    /// On a line the diff adds or changes.
    Changed(Landing),
    /// On an unchanged line a hunk draws.
    Drawn(Landing),
    /// In a changed file, outside its hunks: the hunk opens to show it.
    Revealable(Landing),
    /// In a file the diff does not touch: it opens as a visited file.
    Elsewhere,
    /// Nowhere navigation can go, and why.
    Unreachable(String),
}

impl Place {
    fn in_diff(&self) -> bool {
        matches!(self, Place::Changed(_))
    }
}

/// A file of the model, the column whose body shows it, that body's new
/// side, and the answer's line in what the body shows.
#[derive(Clone, Copy)]
struct Landing {
    index: usize,
    column: Column,
    side: LineSide,
    line: usize,
}

/// Where a jump left from, for `Ctrl+o`.
struct Origin {
    anchor: Option<CommentAnchor>,
    selection: Selection,
    column: Column,
    hide_low: bool,
}

enum Selection {
    File(String),
    Directory(String),
    Position(usize),
}

/// The symbol a lookup asks about.
struct Target {
    path: String,
    /// The version the pane shows, and the symbol's line in it.
    diff: Arc<Diff>,
    line: usize,
    /// The file on disk, which the server reads, and the line there.
    disk: Arc<str>,
    disk_line: usize,
    columns: Range<usize>,
    language: Language,
    name: String,
}

impl Navigation {
    pub(super) fn new(lsp: Option<Lsp>, root: Option<PathBuf>) -> Self {
        Self {
            hover: Hover::new(lsp.clone()),
            lsp,
            root,
            disk: HashMap::new(),
            symbols: HashMap::new(),
            maps: HashMap::new(),
            picked: None,
            pending: None,
            list: None,
            back: Vec::new(),
            visited: Vec::new(),
            warmed: HashSet::new(),
            languages: Vec::new(),
            started: Instant::now(),
            frame: 0,
            busy: false,
        }
    }

    /// The language servers and the workdir they serve, to share with
    /// another view of the same repository.
    pub(super) fn servers(&self) -> Option<(Lsp, PathBuf)> {
        Some((self.lsp.clone()?, self.root.clone()?))
    }

    /// Where the workdir watcher reports changes on disk to the servers.
    pub(super) fn file_events(&self) -> Option<FileEvents> {
        self.lsp.as_ref().map(Lsp::file_events)
    }

    pub(super) fn poll(&self) {
        if let Some(lsp) = &self.lsp {
            lsp.poll();
        }
    }

    /// Forget the jump trail and the visited files it opened: the diff
    /// now holds other files, so the trail leads through a review that
    /// is over.
    pub(super) fn forget_trail(&mut self) {
        self.visited.clear();
        self.back.clear();
        self.list = None;
    }

    /// Add every visited file that is not part of `model` back to it,
    /// read again from disk, after a rebuild.
    pub(super) fn revisit(&self, model: &mut Model) {
        let Some(root) = &self.root else {
            return;
        };
        for path in &self.visited {
            if model.index_of(path).is_none()
                && let Ok(text) = std::fs::read_to_string(root.join(path))
            {
                model.visit(path, &text);
            }
        }
    }

    /// Tell the language servers what the diff now shows for each file.
    pub(super) fn sync(&mut self, model: &Model) {
        self.disk.clear();
        if let Some(lsp) = &self.lsp {
            lsp.sync(model.files.iter().map(|resolved| {
                (
                    crate::sidebar::display_path(&resolved.file),
                    resolved.after.as_str(),
                )
            }));
        }
    }

    /// The file at `path` as it is on disk now.
    fn disk_text(&mut self, path: &str) -> Option<Arc<str>> {
        let full = self.root.as_ref()?.join(path);
        let meta = std::fs::metadata(&full).ok()?;
        let stamp = (meta.len(), meta.modified().ok());
        if let Some(disk) = self.disk.get(path)
            && disk.stamp == stamp
        {
            return Some(Arc::clone(&disk.text));
        }
        let text: Arc<str> = Arc::from(std::fs::read_to_string(&full).ok()?);
        self.disk.insert(
            path.to_string(),
            Disk {
                stamp,
                text: Arc::clone(&text),
            },
        );
        Some(text)
    }

    /// The file on disk at `path` and, when it differs from `shown`
    /// (the version a pane shows), a map from shown lines to disk lines.
    /// `None` when the file is not on disk.
    fn version(&mut self, path: &str, shown: &str) -> Option<(Arc<str>, Option<Arc<LineMap>>)> {
        let disk = self.disk_text(path)?;
        if *disk == *shown {
            return Some((disk, None));
        }
        let key = (fingerprint(shown), fingerprint(&disk));
        if let Some((cached, map)) = self.maps.get(path)
            && *cached == key
        {
            let map = Arc::clone(map);
            return Some((disk, Some(map)));
        }
        let map = Arc::new(LineMap::new(shown, &disk));
        self.maps.insert(path.to_string(), (key, Arc::clone(&map)));
        Some((disk, Some(map)))
    }

    fn symbols(&mut self, path: &str, diff: &Diff, language: Language) -> Arc<Symbols> {
        let key = fingerprint(diff.updated());
        if let Some((cached, symbols)) = self.symbols.get(path)
            && *cached == key
        {
            return Arc::clone(symbols);
        }
        let symbols = Arc::new(Symbols::parse(language, diff.updated()));
        self.symbols
            .insert(path.to_string(), (key, Arc::clone(&symbols)));
        symbols
    }

    /// Whether a server for a language on screen is starting or
    /// indexing, so its mark spins.
    fn loading(&self) -> bool {
        self.lsp.as_ref().is_some_and(|lsp| {
            self.languages
                .iter()
                .any(|&language| matches!(lsp.state(language), State::Loading(_)))
        })
    }

    fn spinner(&self) -> &'static str {
        SPINNER[self.spinner_frame() % SPINNER.len()]
    }

    fn spinner_frame(&self) -> usize {
        (self.started.elapsed().as_millis() / 100) as usize
    }
}

// ---------------------------------------------------------------------------
// Keys and mouse
// ---------------------------------------------------------------------------

/// A key, before Files mode's own bindings. The open list takes every
/// key; otherwise any key cancels a pending lookup, and in the focused
/// diff `h`/`l` and Left/Right move the picked symbol, `Enter` asks for its
/// definition, `r` for its callers, and `i` for its info. Reports
/// whether it took the key.
pub(super) fn handle_key(state: &mut FilesMode, key: KeyCode) -> bool {
    if state.nav.list.is_some() {
        list_key(state, key);
        return true;
    }
    if key == KeyCode::Esc {
        return escape(state);
    }
    cancel(state);
    if state.focus != Focus::Diff {
        return false;
    }
    match key {
        KeyCode::Char('l') | KeyCode::Right => step_symbol(state, 1),
        KeyCode::Char('h') | KeyCode::Left => step_symbol(state, -1),
        KeyCode::Enter => ask(state, Query::Definition),
        KeyCode::Char('r') => ask(state, Query::Callers),
        KeyCode::Char('i') => info(state),
        _ => return false,
    }
    true
}

/// Close the hover popup, as any key but `Esc` does before it acts.
pub(super) fn close_hover(state: &mut FilesMode) {
    state.nav.hover.close();
}

/// A mouse event navigation owns: pointer moves (hover), the wheel over
/// the hover popup, any press while the list is open, and a right click
/// on the diff. Any other event closes the hover popup. Reports whether
/// it took the event.
pub(super) fn mouse(state: &mut FilesMode, mouse: &MouseEvent) -> bool {
    let position = Position::new(mouse.column, mouse.row);
    match mouse.kind {
        MouseEventKind::Moved => {
            pointer_moved(state, position, mouse);
            return true;
        }
        MouseEventKind::ScrollDown if state.nav.hover.scroll(position, 1) => return true,
        MouseEventKind::ScrollUp if state.nav.hover.scroll(position, -1) => return true,
        MouseEventKind::Up(_) => {}
        _ => {
            state.nav.hover.close();
        }
    }
    let press = |button| mouse.kind == MouseEventKind::Down(button);
    if state.nav.list.is_some() {
        if press(MouseButton::Left) || press(MouseButton::Right) {
            list_click(state, mouse);
        }
        return true;
    }
    if press(MouseButton::Right)
        && state
            .diff_rect
            .contains(Position::new(mouse.column, mouse.row))
    {
        state.focus = Focus::Diff;
        let row = row_at(state, mouse);
        click(state, mouse, row, Press::Right);
        return true;
    }
    false
}

/// A left press on the diff: select the line and the symbol under the
/// pointer; a double click (`double`) asks for the definition.
pub(super) fn left_click(state: &mut FilesMode, mouse: &MouseEvent, double: bool) {
    let row = row_at(state, mouse);
    let press = if double { Press::Double } else { Press::Left };
    click(state, mouse, row, press);
}

/// Which press [`click`] handles.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Press {
    Left,
    Double,
    Right,
}

/// The pointer moved to `position`: keep the hover popup while it stays
/// on the symbol or the popup, and otherwise rest on the symbol under it.
fn pointer_moved(state: &mut FilesMode, position: Position, mouse: &MouseEvent) {
    if state.nav.hover.holds(position) {
        return;
    }
    let spot = if state.nav.list.is_none()
        && diff_scrollbar::body_area(state.diff_rect).contains(position)
    {
        spot_at(state, mouse)
    } else {
        None
    };
    state.nav.hover.rest(spot);
}

/// The symbol under the pointer, when it can be asked about.
fn spot_at(state: &mut FilesMode, mouse: &MouseEvent) -> Option<Spot> {
    let row = row_at(state, mouse);
    let column = state.shown();
    let pane = state.pane(column);
    let scroll = pane.cursor.scroll;
    let (first, anchor, offset) = pane
        .line_at(row)
        .map(|(first, anchor, offset)| (first, anchor.clone(), offset))?;
    let body = diff_scrollbar::body_area(state.diff_rect);
    let x = mouse.column.saturating_sub(body.x) as usize;
    let (target, _) = target_on(state, anchor.clone(), |text, ranges| {
        let byte = byte_at(text, offset, x, body.width as usize)?;
        ranges.iter().position(|range| range.contains(&byte))
    })
    .ok()?;
    let top = first.checked_sub(scroll)?;
    Some(spot(target, anchor, body, top))
}

/// `target`'s symbol on `anchor`'s line as hover sees it, the line's
/// first row drawn `top` rows into `body`.
fn spot(target: Target, anchor: CommentAnchor, body: Rect, top: usize) -> Spot {
    let text = line_text(&target.diff, target.line);
    let cells = line_cells(text, target.columns.clone(), body.width as usize)
        .into_iter()
        .filter_map(|(row, columns)| {
            let y = body.y as usize + top + row;
            (y < body.bottom() as usize)
                .then(|| Rect::new(body.x + columns.start, y as u16, columns.len() as u16, 1))
        })
        .collect();
    Spot {
        anchor,
        columns: target.columns,
        cells,
        name: target.name,
        language: target.language,
        highlight: target.diff.highlight().map(str::to_string),
        path: target.path,
        disk: target.disk,
        disk_line: target.disk_line,
    }
}

/// `i`: show the server's info about the picked symbol.
fn info(state: &mut FilesMode) {
    let target = match target(state) {
        Ok((target, _)) => target,
        Err(message) => {
            state.status = Some(message);
            return;
        }
    };
    let Some(anchor) = cursor_anchor(state) else {
        return;
    };
    let Some((body, top)) = state.pane(state.shown()).cursor_on_screen(state.diff_rect) else {
        return;
    };
    if let Err(message) = state.nav.hover.open(spot(target, anchor, body, top)) {
        state.status = Some(message);
    }
}

/// The diff window row under the pointer.
fn row_at(state: &FilesMode, mouse: &MouseEvent) -> usize {
    let content_y = mouse
        .row
        .saturating_sub(state.diff_rect.y)
        .saturating_sub(1) as usize;
    state.pane(state.shown()).cursor.scroll + content_y
}

/// Move the picked symbol `step` symbols along the cursor's line.
pub(super) fn step_symbol(state: &mut FilesMode, step: isize) {
    match target(state) {
        Ok((target, ranges)) => {
            let current = ranges
                .iter()
                .position(|range| *range == target.columns)
                .unwrap_or(0);
            let next = current.saturating_add_signed(step).min(ranges.len() - 1);
            if let Some(anchor) = cursor_anchor(state) {
                state.nav.picked = Some((anchor, next));
            }
        }
        Err(message) => state.status = Some(message),
    }
}

/// Ask the language server `query` about the picked symbol.
pub(super) fn ask(state: &mut FilesMode, query: Query) {
    cancel(state);
    let target = match target(state) {
        Ok((target, _)) => target,
        Err(message) => {
            state.status = Some(message);
            return;
        }
    };
    let Some(lsp) = state.nav.lsp.clone() else {
        return;
    };
    let asked = lsp.ask(
        query,
        Symbol {
            language: target.language,
            path: &target.path,
            text: &target.disk,
            line: target.disk_line,
            columns: target.columns.clone(),
        },
    );
    match asked {
        Ok(ticket) => {
            state.nav.pending = Some(Pending {
                ticket,
                query,
                name: target.name,
                path: target.path,
                line: target.disk_line,
            });
        }
        Err(message) => state.status = Some(message),
    }
}

/// Drop the pending lookup, if any, and close the hover popup.
pub(super) fn cancel(state: &mut FilesMode) {
    state.nav.hover.close();
    if let Some(pending) = state.nav.pending.take()
        && let Some(lsp) = &state.nav.lsp
    {
        lsp.cancel(pending.ticket);
    }
}

/// `Esc`: close the hover popup, the list, or cancel the lookup.
/// Reports whether there was any.
pub(super) fn escape(state: &mut FilesMode) -> bool {
    if state.nav.hover.close() {
        return true;
    }
    if state.nav.list.take().is_some() {
        return true;
    }
    if state.nav.pending.is_some() {
        cancel(state);
        state.status = Some("Cancelled".to_string());
        return true;
    }
    false
}

/// A key while the list is open: it owns every key.
fn list_key(state: &mut FilesMode, key: KeyCode) {
    let Some(list) = state.nav.list.as_mut() else {
        return;
    };
    match key {
        KeyCode::Char('j') | KeyCode::Down => {
            list.selected = (list.selected + 1).min(list.entries.len() - 1);
        }
        KeyCode::Char('k') | KeyCode::Up => list.selected = list.selected.saturating_sub(1),
        KeyCode::Enter => {
            let list = state.nav.list.take().expect("checked above");
            let entry = list.entries.into_iter().nth(list.selected);
            if let Some(entry) = entry {
                go(state, entry);
            }
        }
        KeyCode::Esc => state.nav.list = None,
        _ => {}
    }
}

/// A left or right press while the list is open: a row goes there, a
/// press outside closes it.
fn list_click(state: &mut FilesMode, mouse: &MouseEvent) {
    let Some(list) = state.nav.list.take() else {
        return;
    };
    let inside = list.area.contains(Position::new(mouse.column, mouse.row));
    let hit = list
        .rows
        .iter()
        .find(|(row, _)| *row == mouse.row)
        .map(|(_, entry)| *entry);
    match hit {
        Some(entry) if inside => {
            if let Some(entry) = list.entries.into_iter().nth(entry) {
                go(state, entry);
            }
        }
        _ if inside => state.nav.list = Some(list),
        _ => {}
    }
}

/// A press on the diff pane at window row `row`, screen column `column`:
/// select the line and the symbol under the pointer. A double click asks
/// for the definition and a right click for the callers.
fn click(state: &mut FilesMode, mouse: &MouseEvent, row: usize, press: Press) {
    cancel(state);
    let column = state.shown();
    let Some((first, anchor, offset)) = state
        .pane(column)
        .line_at(row)
        .map(|(first, anchor, offset)| (first, anchor.clone(), offset))
    else {
        return;
    };
    state.pane_mut(column).select_row(first);
    let body = diff_scrollbar::body_area(state.diff_rect);
    let x = mouse.column.saturating_sub(body.x) as usize;
    if let Ok((target, ranges)) = target(state) {
        let text = line_text(&target.diff, target.line);
        if let Some(byte) = byte_at(text, offset, x, body.width as usize)
            && let Some(index) = ranges.iter().position(|range| range.contains(&byte))
        {
            state.nav.picked = Some((anchor, index));
            match press {
                Press::Double => ask(state, Query::Definition),
                Press::Right => ask(state, Query::Callers),
                Press::Left => {}
            }
        }
    }
}

/// `Ctrl+o`: go back to where the last jump left from.
pub(super) fn back(state: &mut FilesMode) {
    cancel(state);
    state.nav.list = None;
    let Some(origin) = state.nav.back.pop() else {
        state.status = Some("No earlier place to go back to".to_string());
        return;
    };
    let sidebar_height = sidebar_height(state);
    drop_unreachable_visits(state, &origin, sidebar_height);
    if state.hide_low != origin.hide_low {
        state.hide_low = origin.hide_low;
        state.rebuild_sidebar(sidebar_height);
    }
    let moved = match &origin.selection {
        Selection::File(path) => match state.model.index_of(path) {
            Some(index) => {
                state.sidebar.nearest_file_index() != Some(index)
                    && state.sidebar.select_file_index(index, sidebar_height)
            }
            None => false,
        },
        Selection::Directory(path) => {
            state.sidebar.selected_directory_path().as_deref() != Some(path)
                && state.sidebar.select_directory_path(path, sidebar_height)
        }
        Selection::Position(position) => {
            let moved = state.sidebar.selected() != *position;
            state.sidebar.set_selected(*position, sidebar_height);
            moved
        }
    };
    if moved {
        state.snap_diff_to_selected_file();
    }
    state.column = origin.column;
    state.focus = Focus::Diff;
    state.nav.picked = None;
    if let Some(anchor) = origin.anchor {
        let height = diff_height(state);
        state.pane_mut(origin.column).jump_to(anchor, height);
    }
}

// ---------------------------------------------------------------------------
// Answers
// ---------------------------------------------------------------------------

/// Collect a lookup's answer and advance the spinner. Returns whether the
/// screen changed and whether more is to come.
pub(super) fn tick(state: &mut FilesMode) -> (bool, bool) {
    let mut changed = false;
    let answer = match (&state.nav.pending, &state.nav.lsp) {
        (Some(pending), Some(lsp)) => lsp.answer(&pending.ticket),
        _ => None,
    };
    if let Some(answer) = answer {
        let pending = state.nav.pending.take().expect("checked above");
        answered(state, pending, answer);
        changed = true;
    }
    let hover = state.nav.hover.tick();
    changed |= hover.changed;
    if let Some(message) = hover.status {
        state.status = Some(message);
        changed = true;
    }
    let busy =
        state.nav.pending.is_some() || state.nav.hover.asking().is_some() || state.nav.loading();
    changed |= busy != state.nav.busy;
    state.nav.busy = busy;
    if busy {
        let frame = state.nav.spinner_frame();
        changed |= frame != state.nav.frame;
        state.nav.frame = frame;
    }
    (changed, busy || hover.pending)
}

fn answered(state: &mut FilesMode, pending: Pending, answer: Result<Reply, String>) {
    let mut found = match answer {
        Ok(Reply::Places(found)) => found,
        Ok(Reply::Info(_)) => return,
        Err(message) => {
            state.status = Some(message);
            return;
        }
    };
    if pending.query == Query::Definition {
        let asked = found.len();
        found.retain(|found| found.path != pending.path || found.line != pending.line);
        // Asked on the definition itself: who uses it is the question left.
        if found.is_empty() && asked > 0 {
            ask(state, Query::Callers);
            state.status = Some(format!(
                "Already at the definition of {}; showing its callers",
                pending.name
            ));
            return;
        }
    }
    let mut entries: Vec<Entry> = found
        .into_iter()
        .map(|found| {
            let place = place(state, &found);
            let code = code(state, &found, &place);
            Entry { found, place, code }
        })
        .collect();
    if let [_] = entries.as_slice() {
        let entry = entries.pop().expect("one entry");
        go(state, entry);
        return;
    }
    entries.sort_by_key(|entry| !entry.place.in_diff());
    let title = match pending.query {
        Query::Definition => format!("Definitions of {}", pending.name),
        Query::Callers | Query::Hover => format!("Callers of {}", pending.name),
    };
    state.nav.list = Some(List {
        title,
        entries,
        selected: 0,
        area: Rect::default(),
        rows: Vec::new(),
    });
}

/// Where `found` lands, given what the model shows and what is on disk.
fn place(state: &mut FilesMode, found: &Found) -> Place {
    if Path::new(&found.path).is_absolute() {
        return Place::Unreachable(format!("{} is outside the repository", found.path));
    }
    let Some(index) = state.model.index_of(&found.path) else {
        return Place::Elsewhere;
    };
    let column = column_of(&state.model, index);
    let Some((diff, side)) = body(&state.model, index, column) else {
        return Place::Unreachable(format!("{} has no text to show", found.path));
    };
    let Some((_, map)) = state.nav.version(&found.path, diff.updated()) else {
        return Place::Unreachable(format!("{} isn't on disk", found.path));
    };
    let line = match map {
        None => found.line,
        Some(map) => match map.backward(found.line) {
            Some(line) => line,
            None => {
                return Place::Unreachable(format!(
                    "{}:{} has changed since this version",
                    found.path, found.line
                ));
            }
        },
    };
    let landing = Landing {
        index,
        column,
        side,
        line,
    };
    let anchor = anchor(&found.path, side, line);
    match state.model.line(&anchor) {
        Some((_, LineKind::Added)) => Place::Changed(landing),
        Some(_) => Place::Drawn(landing),
        None => Place::Revealable(landing),
    }
}

/// The column that lists file `index`: the unstaged view whenever the
/// file has one, since that is the disk's text.
fn column_of(model: &Model, index: usize) -> Column {
    let staged_only = model.splits.get(index).is_none_or(Option::is_none)
        && model
            .stage(index)
            .is_some_and(|stage| stage.is_staged() && !stage.is_unstaged());
    if staged_only {
        Column::Staged
    } else {
        Column::Unstaged
    }
}

/// The line of code an answer points at, trimmed, for the list.
fn code(state: &mut FilesMode, found: &Found, place: &Place) -> String {
    let (text, line): (Option<Arc<str>>, usize) = match place {
        Place::Changed(landing) | Place::Drawn(landing) | Place::Revealable(landing) => (
            body(&state.model, landing.index, landing.column)
                .map(|(diff, _)| Arc::from(diff.updated())),
            landing.line,
        ),
        _ => (state.nav.disk_text(&found.path), found.line),
    };
    text.and_then(|text| {
        text.lines()
            .nth(line.saturating_sub(1))
            .map(|line| line.trim().to_string())
    })
    .unwrap_or_default()
}

/// Act on one answer: jump to it, open its hunk, or visit its file.
fn go(state: &mut FilesMode, entry: Entry) {
    let Entry { found, place, .. } = entry;
    let line = match &place {
        Place::Changed(landing) | Place::Drawn(landing) | Place::Revealable(landing) => {
            landing.line
        }
        _ => found.line,
    };
    let at = format!("{}:{line}", found.path);
    match place {
        Place::Unreachable(message) => state.status = Some(message),
        Place::Changed(landing) | Place::Drawn(landing) => {
            jump(state, landing, anchor(&found.path, landing.side, line));
            state.status = Some(format!("{at}  ·  Ctrl+o to go back"));
        }
        Place::Revealable(landing) => {
            if state
                .model
                .reshape(landing.index, landing.column, |diff| diff.reveal(line))
            {
                state.staged.cache.refresh(landing.index);
                state.unstaged.cache.refresh(landing.index);
            }
            jump(state, landing, anchor(&found.path, landing.side, line));
            state.status = Some(format!(
                "{at}  ·  opened like z, x folds it  ·  Ctrl+o to go back"
            ));
        }
        Place::Elsewhere => {
            let Some(text) = state.nav.disk_text(&found.path) else {
                state.status = Some(format!("Can't read {}", found.path));
                return;
            };
            let index = state.model.visit(&found.path, &text);
            if !state.nav.visited.contains(&found.path) {
                state.nav.visited.push(found.path.clone());
            }
            let height = sidebar_height(state);
            state.rebuild_sidebar(height);
            let landing = Landing {
                index,
                column: Column::Unstaged,
                side: LineSide::New,
                line,
            };
            jump(state, landing, anchor(&found.path, LineSide::New, line));
            state.status = Some(format!("{at}  ·  not in this diff  ·  Ctrl+o to go back"));
        }
    }
}

/// Remember where the cursor is, then show `landing`'s file and put the
/// cursor on `target`.
fn jump(state: &mut FilesMode, landing: Landing, target: CommentAnchor) {
    remember(state);
    let sidebar_height = sidebar_height(state);
    if state.hidden().get(landing.index).copied().unwrap_or(false) {
        state.hide_low = false;
        state.rebuild_sidebar(sidebar_height);
    }
    let shown = state
        .display_order
        .iter()
        .position(|&index| index == landing.index)
        .is_some_and(|position| {
            state
                .selection()
                .is_some_and(|range| range.contains(&position))
        });
    if !shown {
        state
            .sidebar
            .select_file_index(landing.index, sidebar_height);
        state.snap_diff_to_selected_file();
    }
    state.column = landing.column;
    state.focus = Focus::Diff;
    state.nav.picked = None;
    let height = diff_height(state);
    state.pane_mut(landing.column).jump_to(target, height);
}

/// Drop the visited files no jump leads back to once `Ctrl+o` returns to
/// `origin`: they were detours off the trail.
fn drop_unreachable_visits(state: &mut FilesMode, origin: &Origin, sidebar_height: usize) {
    let reachable = |path: &String| {
        state
            .nav
            .back
            .iter()
            .chain(std::iter::once(origin))
            .any(|origin| origin.leads_to(path))
    };
    let dropped: Vec<String> = state
        .nav
        .visited
        .iter()
        .filter(|path| !reachable(path))
        .cloned()
        .collect();
    if dropped.is_empty() {
        return;
    }
    state.nav.visited.retain(|path| !dropped.contains(path));
    let moved = state.model.unvisit(&dropped);
    state.staged.carry_over(&moved);
    state.unstaged.carry_over(&moved);
    state.rebuild_sidebar(sidebar_height);
}

impl Origin {
    /// Whether going back to this place shows file `path`.
    fn leads_to(&self, path: &str) -> bool {
        self.anchor
            .as_ref()
            .is_some_and(|anchor| anchor.path == path)
            || matches!(&self.selection, Selection::File(file) if file == path)
    }
}

fn remember(state: &mut FilesMode) {
    let selection = if let Some(path) = state.sidebar.selected_directory_path() {
        Selection::Directory(path)
    } else if let Some(index) = state.sidebar.selected_file_index() {
        Selection::File(crate::sidebar::display_path(&state.model.files[index].file).to_string())
    } else {
        Selection::Position(state.sidebar.selected())
    };
    let column = state.shown();
    let origin = Origin {
        anchor: state.pane(column).cursor_anchor().cloned(),
        selection,
        column,
        hide_low: state.hide_low,
    };
    if state.nav.back.len() == BACK_LIMIT {
        state.nav.back.remove(0);
    }
    state.nav.back.push(origin);
}

// ---------------------------------------------------------------------------
// The picked symbol
// ---------------------------------------------------------------------------

/// The picked symbol on the cursor's line and every symbol of that line,
/// or why there is none to ask about.
fn target(state: &mut FilesMode) -> Result<(Target, Vec<Range<usize>>), String> {
    let anchor = cursor_anchor(state).ok_or("Move the cursor to a diff line first")?;
    let picked = match &state.nav.picked {
        Some((picked, index)) if *picked == anchor => Some(*index),
        _ => None,
    };
    target_on(state, anchor, |text, ranges| {
        Some(match picked {
            Some(index) => index.min(ranges.len() - 1),
            None => first_call(text, ranges),
        })
    })
}

/// The symbol `pick` chooses on `anchor`'s line (given the line's text
/// and its symbols' byte ranges) and every symbol of that line, or why
/// there is none to ask about.
fn target_on(
    state: &mut FilesMode,
    anchor: CommentAnchor,
    pick: impl FnOnce(&str, &[Range<usize>]) -> Option<usize>,
) -> Result<(Target, Vec<Range<usize>>), String> {
    let index = state.model.index_of(&anchor.path).ok_or("")?;
    let column = state.shown();
    let (diff, side) =
        body(&state.model, index, column).ok_or("This file has no text to navigate")?;
    if anchor.side != side {
        return Err("Removed lines aren't on disk; pick a line that is".to_string());
    }
    let (disk, map) = state
        .nav
        .version(&anchor.path, diff.updated())
        .ok_or_else(|| format!("{} isn't on disk", anchor.path))?;
    let disk_line = match &map {
        None => anchor.line,
        Some(map) => map
            .forward(anchor.line)
            .ok_or("This line has changed since this version")?,
    };
    let lsp = state
        .nav
        .lsp
        .as_ref()
        .ok_or("Go to definition needs a repository")?;
    let language = diff.language().ok_or("No language server for this file")?;
    if !lsp.serves(language) {
        return Err(format!("No language server for {}", language.name()));
    }
    let symbols = state.nav.symbols(&anchor.path, &diff, language);
    let ranges = symbols.on_line(anchor.line);
    if ranges.is_empty() {
        return Err("No symbol on this line".to_string());
    }
    let text = line_text(&diff, anchor.line);
    let picked = pick(text, &ranges).ok_or("No symbol there")?;
    let columns = ranges[picked].clone();
    let name = text[columns.clone()].to_string();
    Ok((
        Target {
            path: anchor.path,
            diff,
            line: anchor.line,
            disk,
            disk_line,
            columns,
            language,
            name,
        },
        ranges,
    ))
}

/// The first symbol followed by `(`: usually what a reader wants to
/// follow. The first symbol when nothing on the line is a call.
fn first_call(text: &str, ranges: &[Range<usize>]) -> usize {
    ranges
        .iter()
        .position(|range| text[range.end..].trim_start().starts_with('('))
        .unwrap_or(0)
}

fn cursor_anchor(state: &FilesMode) -> Option<CommentAnchor> {
    state.pane(state.shown()).cursor_anchor().cloned()
}

/// File `index`'s text body in `column`, and the side its new lines are
/// numbered on.
fn body(model: &Model, index: usize, column: Column) -> Option<(Arc<Diff>, LineSide)> {
    let view = model.view(index, column);
    match view.body {
        FileBody::Diff(diff) => Some((Arc::clone(diff), view.numbering.new)),
        _ => None,
    }
}

fn line_text(diff: &Diff, line: usize) -> &str {
    diff.updated()
        .lines()
        .nth(line.saturating_sub(1))
        .unwrap_or_default()
}

fn anchor(path: &str, side: LineSide, line: usize) -> CommentAnchor {
    CommentAnchor {
        path: path.to_string(),
        side,
        line,
    }
}

fn fingerprint(text: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

fn diff_height(state: &FilesMode) -> usize {
    diff_scrollbar::body_area(state.diff_rect).height.max(1) as usize
}

fn sidebar_height(state: &FilesMode) -> usize {
    state.sidebar_rect.height.saturating_sub(2).max(1) as usize
}

// ---------------------------------------------------------------------------
// Drawing
// ---------------------------------------------------------------------------

/// The footer while a lookup runs, in place of the pane's own. `None`
/// when no lookup is pending.
pub(super) fn footer(nav: &Navigation) -> Option<String> {
    if let Some(name) = nav.hover.asking() {
        return Some(format!("{} Finding info for {name}", nav.spinner()));
    }
    let pending = nav.pending.as_ref()?;
    let what = match pending.query {
        Query::Definition => "definition",
        Query::Callers => "callers",
        Query::Hover => "info",
    };
    Some(format!(
        "{} Finding {what} of {}",
        nav.spinner(),
        pending.name
    ))
}

/// Note the languages of `files` (the ones the pane shows) and start
/// their servers, so lookups answer sooner and each header can show its
/// server's state.
pub(super) fn observe(state: &mut FilesMode, files: &[usize]) {
    let column = state.shown();
    let mut languages: Vec<Language> = Vec::new();
    for language in files
        .iter()
        .filter_map(|&index| body(&state.model, index, column)?.0.language())
    {
        if !languages.contains(&language) {
            languages.push(language);
        }
    }
    if let Some(lsp) = &state.nav.lsp {
        for &language in &languages {
            if lsp.serves(language) && state.nav.warmed.insert(language) {
                lsp.warm(language);
            }
        }
    }
    state.nav.languages = languages;
}

/// Underline the picked symbol on the cursor's line, and draw the list.
pub(super) fn draw(
    state: &mut FilesMode,
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    focused: bool,
    theme: &Theme,
) {
    if focused
        && state.focus == Focus::Diff
        && let Ok((target, _)) = target(state)
        && let Some((body, offset)) = state.pane(state.shown()).cursor_on_screen(area)
    {
        let text = line_text(&target.diff, target.line);
        let style = Style::default().add_modifier(Modifier::UNDERLINED | Modifier::BOLD);
        for (row, columns) in line_cells(text, target.columns.clone(), body.width as usize) {
            let y = body.y as usize + offset + row;
            if y < body.bottom() as usize {
                let cells = Rect::new(body.x + columns.start, y as u16, columns.len() as u16, 1);
                frame.buffer_mut().set_style(cells, style);
            }
        }
    }
    draw_border_badge(state, frame, area, theme);
    state.nav.hover.draw(frame, area, theme);
    if let Some(list) = state.nav.list.as_mut() {
        draw_list(frame, list, theme);
    }
}

/// The right end of the diff pane's top border for file `index`: its
/// language badge (icon and name), then its language server's state. A
/// green `●` when ready, a spinner (and percentage) while it starts or
/// indexes, a red `✕` when it failed, nothing when there is no server.
fn border_badge(state: &FilesMode, index: usize, theme: &Theme) -> Vec<Span<'static>> {
    let Some((diff, _)) = body(&state.model, index, state.shown()) else {
        return Vec::new();
    };
    let path = crate::sidebar::display_path(&state.model.files[index].file);
    let Some(badge) = SyntaxBadge::new(
        diff.language(),
        diff.highlight(),
        path,
        IconMode::from_env(),
        theme,
    ) else {
        return Vec::new();
    };
    let mut spans = badge.spans().to_vec();
    let mark = match (&state.nav.lsp, diff.language()) {
        (Some(lsp), Some(language)) => match lsp.state(language) {
            State::Off => None,
            State::Ready => Some(("●".to_string(), theme.status_added)),
            State::Loading(percent) => {
                let spinner = state.nav.spinner();
                let mark = percent.map_or(spinner.to_string(), |p| format!("{spinner} {p}%"));
                Some((mark, theme.status_partial))
            }
            State::Failed => Some(("✕".to_string(), theme.status_deleted)),
        },
        _ => None,
    };
    if let Some((mark, color)) = mark {
        spans.push(Span::raw(" "));
        spans.push(Span::styled(
            mark,
            Style::default()
                .fg(rgb_to_color(color))
                .add_modifier(Modifier::BOLD),
        ));
    }
    spans
}

/// Columns the right end of the border takes for file `index`, with the
/// filter funnel when it shows: what the title leaves free.
pub(super) fn border_badge_width(state: &FilesMode, index: usize, theme: &Theme) -> usize {
    let badge: usize = border_badge(state, index, theme)
        .iter()
        .map(|span| display_width(&span.content))
        .sum();
    let funnel = if state.hide_low_hunks {
        display_width(filter_icon(IconMode::from_env())) + 1
    } else {
        0
    };
    badge + funnel + 2
}

/// Draw the right end of the diff pane's top border for the file the
/// pane is on, left of the filter funnel when there is one, and only
/// over the border's rule, so the title keeps priority.
fn draw_border_badge(state: &FilesMode, frame: &mut ratatui::Frame<'_>, area: Rect, theme: &Theme) {
    let Some(index) = state.pane(state.shown()).current_file(&state.model, area) else {
        return;
    };
    let spans = border_badge(state, index, theme);
    if spans.is_empty() {
        return;
    }
    let width: usize = spans
        .iter()
        .map(|span| display_width(&span.content))
        .sum::<usize>()
        + 2;
    let funnel = if state.hide_low_hunks {
        display_width(filter_icon(IconMode::from_env())) + 1
    } else {
        0
    };
    let Some(end) = (area.right() as usize).checked_sub(2 + funnel) else {
        return;
    };
    let Some(start) = end.checked_sub(width) else {
        return;
    };
    let buffer = frame.buffer_mut();
    let y = area.y;
    let on_rule = (start.saturating_sub(1).max(area.x as usize + 1)..end)
        .all(|x| buffer[(x as u16, y)].symbol() == "─");
    if !on_rule {
        return;
    }
    let border = buffer[(start as u16, y)].style();
    let mut x = start as u16;
    buffer.set_string(x, y, " ", border);
    x += 1;
    for span in spans {
        let style = border.patch(span.style);
        buffer.set_string(x, y, &span.content, style);
        x += display_width(&span.content) as u16;
    }
    buffer.set_string(x, y, " ", border);
}

fn draw_list(frame: &mut ratatui::Frame<'_>, list: &mut List, theme: &Theme) {
    let area = frame.area();
    let muted = Style::default().fg(rgb_to_color(theme.muted));
    let header = muted.add_modifier(Modifier::BOLD);
    let path_style = Style::default().fg(rgb_to_color(theme.border_active));
    let caller_width = list
        .entries
        .iter()
        .map(|entry| entry.found.caller.as_deref().map_or(0, str::len))
        .max()
        .unwrap_or(0);
    let path_width = list
        .entries
        .iter()
        .map(|entry| entry.found.path.len() + entry.found.line.to_string().len() + 1)
        .max()
        .unwrap_or(0);

    let mut lines: Vec<(Line<'static>, Option<usize>)> = vec![(Line::from(""), None)];
    let mut group: Option<bool> = None;
    for (index, entry) in list.entries.iter().enumerate() {
        let in_diff = entry.place.in_diff();
        if group != Some(in_diff) {
            if group.is_some() {
                lines.push((Line::from(""), None));
            }
            let label = if in_diff {
                "In this diff"
            } else {
                "Outside the diff"
            };
            lines.push((Line::from(Span::styled(label, header)), None));
            group = Some(in_diff);
        }
        let selected = index == list.selected;
        let base = if selected {
            Style::default().bg(rgb_to_color(theme.selection_bg))
        } else {
            Style::default()
        };
        let marker = if selected { "› " } else { "  " };
        let caller = entry.found.caller.as_deref().unwrap_or_default();
        let place = format!("{}:{}", entry.found.path, entry.found.line);
        let mut spans = vec![Span::styled(marker.to_string(), base)];
        if caller_width > 0 {
            spans.push(Span::styled(format!("{caller:<caller_width$}  "), base));
        }
        spans.push(Span::styled(
            format!("{place:<path_width$}  "),
            base.patch(path_style),
        ));
        spans.push(Span::styled(entry.code.clone(), base));
        lines.push((Line::from(spans), Some(index)));
    }
    lines.push((Line::from(""), None));
    lines.push((
        Line::from(Span::styled(
            "j/k move  ·  Enter or click go  ·  Esc close",
            muted,
        )),
        None,
    ));

    let width = area.width.clamp(20, 100).min(area.width);
    let height = (lines.len() as u16 + 2).min(area.height);
    let popup = Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    );
    let inner_height = height.saturating_sub(2) as usize;
    let selected_line = lines
        .iter()
        .position(|(_, entry)| *entry == Some(list.selected))
        .unwrap_or(0);
    let skip = (selected_line + 2).saturating_sub(inner_height);
    list.area = popup;
    list.rows = lines
        .iter()
        .enumerate()
        .skip(skip)
        .take(inner_height)
        .filter_map(|(line, (_, entry))| Some((popup.y + 1 + (line - skip) as u16, (*entry)?)))
        .collect();

    frame.render_widget(Clear, popup);
    let title = Line::from(format!("─{}─", list.title));
    let block = pane_block_with_title_line(title, pane_border_color(true, theme), None);
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    let visible: Vec<Line<'static>> = lines
        .into_iter()
        .skip(skip)
        .take(inner_height)
        .map(|(line, _)| line)
        .collect();
    let padded = Rect::new(
        inner.x + 1,
        inner.y,
        inner.width.saturating_sub(2),
        inner.height,
    );
    frame.render_widget(Paragraph::new(visible), padded);
}

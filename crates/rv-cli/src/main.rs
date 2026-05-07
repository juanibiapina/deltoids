//! rv: render a git diff with the deltoids look in a scrollable TUI.
//!
//! Mirrors the input pipeline of the `deltoids` CLI exactly: read a
//! unified diff from stdin, parse it, resolve before/after blob content
//! against the local repo, and compute per-file [`Diff`]s. Instead of
//! emitting ANSI text for `less`, render hunks as ratatui
//! [`Line<'static>`] values and scroll them in an alternate screen.
//!
//! Usage:
//!
//! ```sh
//! git diff | rv
//! ```
//!
//! Layout:
//!
//! - Left sidebar — file tree with status badges, nerd icons, and
//!   per-file line-delta counts (lazygit-inspired). Selecting a file
//!   scrolls the diff pane to that file's header.
//! - Right pane — the deltoids diff renderer, scrollable.
//!
//! Keys:
//!
//! - `Tab` / `1` / `2` — focus sidebar / diff.
//! - `j`/`k` — move selection (sidebar) or scroll one line (diff).
//! - `Shift+J`/`Shift+K` — scroll diff three lines, regardless of focus.
//! - `PgDn`/`PgUp` / `Space` — page (current focus).
//! - `g`/`G` / `Home`/`End` — jump to top/bottom (current focus).
//! - `v` — cycle view mode: Full diff → Signatures only → Summary.
//! - `p` — toggle public-only filter (drop files with no public changes).
//! - `q`/`Esc` — quit.
//!
//! Set `RV_NO_ICONS=1` to disable nerd-font glyphs in the sidebar.

use std::io::{self, IsTerminal, Read, Write};

use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Margin, Rect};
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::Paragraph;

use deltoids::Language;
use deltoids::content::SideContent;
use deltoids::parse::{FileDiff, GitDiff};
use deltoids::render_tui::{
    self, pane_block_with_footer, pane_border_color, pane_inner_height, render_pane_scrollbar,
    rgb_to_color,
};
use deltoids::structural::{
    LineSpan, OutlineEntry, OutlineStatus, StructuralChange, StructuralDiff, SummaryOptions,
    Visibility, format_summary_with, outline,
};
use deltoids::{Diff, LineKind, Theme, content, git};
use ratatui::text::Span;
use unicode_width::UnicodeWidthStr;

mod sidebar;

use sidebar::{Sidebar, SidebarFile};

const SCROLL_STEP_SMALL: usize = 1;
const SCROLL_STEP_LARGE: usize = 3;

/// Default sidebar width in columns, *including the two border
/// columns* (clamped against the terminal width at draw time). Picked
/// to fit a typical "crates/deltoids/src/" + file row without
/// truncation: outer 38 = inner 36.
const DEFAULT_SIDEBAR_WIDTH: u16 = 38;
/// Below this terminal width the sidebar is hidden entirely.
const MIN_TERMINAL_WIDTH_FOR_SIDEBAR: u16 = 80;

fn main() {
    if let Err(err) = run() {
        eprintln!("rv: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut input = String::new();
    io::stdin()
        .read_to_string(&mut input)
        .map_err(|err| format!("failed to read stdin: {err}"))?;

    if input.is_empty() {
        return Ok(());
    }

    if !io::stdout().is_terminal() {
        return Err(
            "stdout must be a terminal (rv is interactive); pipe diffs into rv, not out of it"
                .to_string(),
        );
    }

    let theme = Theme::load();
    let parsed = GitDiff::parse(&input);
    let repo = git::Repo::discover();
    let resolved = resolve(&parsed, repo.as_ref())?;
    let diffs = precompute_diffs(&resolved);
    let structurals = precompute_structurals(&resolved);
    let outlines = precompute_outlines(&resolved);

    run_tui(&resolved, &diffs, &structurals, &outlines, &theme)
}

/// One file's resolved content, ready for rendering.
#[cfg_attr(test, derive(Debug))]
struct ResolvedFile<'a> {
    file: &'a FileDiff,
    before: String,
    after: String,
}

/// Resolve content for every file. Returns the resolved files on success,
/// or a string describing the first missing blob on failure.
fn resolve<'a>(
    parsed: &'a GitDiff,
    repo: Option<&git::Repo>,
) -> Result<Vec<ResolvedFile<'a>>, String> {
    let mut files = Vec::with_capacity(parsed.files.len());

    for file in &parsed.files {
        let resolved = content::retrieve(file, repo);
        let before = match resolved.before {
            SideContent::Resolved(s) => s,
            SideContent::Absent => String::new(),
            SideContent::Missing { hash } => {
                return Err(missing_blob_message(&hash, display_path(file)));
            }
        };
        let after = match resolved.after {
            SideContent::Resolved(s) => s,
            SideContent::Absent => String::new(),
            SideContent::Missing { hash } => {
                return Err(missing_blob_message(&hash, display_path(file)));
            }
        };
        files.push(ResolvedFile {
            file,
            before,
            after,
        });
    }

    Ok(files)
}

fn missing_blob_message(hash: &str, path: &str) -> String {
    format!(
        "missing index blob {hash} for {path} \u{2014} not found in local repository\n\
         hint: fetch the source ref (e.g. `git fetch <remote> <ref>`) and try again"
    )
}

use sidebar::display_path;

/// Compute one [`Diff`] per resolved file. Done once at startup so the
/// diff pane and the sidebar share the same line-count totals.
fn precompute_diffs(files: &[ResolvedFile<'_>]) -> Vec<Diff> {
    files
        .iter()
        .map(|f| Diff::compute(&f.before, &f.after, display_path(f.file)))
        .collect()
}

/// Compute one [`StructuralDiff`] per resolved file. Cached so the
/// view-mode toggle is instant — no reparsing on every cycle.
fn precompute_structurals(files: &[ResolvedFile<'_>]) -> Vec<StructuralDiff> {
    files
        .iter()
        .map(|f| StructuralDiff::compute(&f.before, &f.after, display_path(f.file)))
        .collect()
}

/// Compute one outline per resolved file. Cached for the same reason
/// as `precompute_structurals`.
fn precompute_outlines(files: &[ResolvedFile<'_>]) -> Vec<Vec<OutlineEntry>> {
    files
        .iter()
        .map(|f| outline(&f.before, &f.after, display_path(f.file)))
        .collect()
}

/// True when the file's structural diff has at least one change
/// touching a public symbol. Used by the public-only filter to drop
/// files entirely.
fn file_has_public_change(s: &StructuralDiff) -> bool {
    s.public_changes().next().is_some()
}

/// Sum added/deleted line counts across all hunks of one diff.
fn count_deltas(diff: &Diff) -> (usize, usize) {
    let mut added = 0;
    let mut deleted = 0;
    for hunk in diff.hunks() {
        for line in &hunk.lines {
            match line.kind {
                LineKind::Added => added += 1,
                LineKind::Removed => deleted += 1,
                LineKind::Context => {}
            }
        }
    }
    (added, deleted)
}

// ---------------------------------------------------------------------------
// View construction
// ---------------------------------------------------------------------------

/// Result of laying out all files into a single scrollable line stream.
struct DiffView {
    lines: Vec<Line<'static>>,
    /// `file_offsets[i]` is the row in `lines` where file `i`'s header
    /// starts. Used by the sidebar to scroll the diff pane in sync with
    /// file selection.
    file_offsets: Vec<usize>,
}

/// Build the right pane as a flat list of ratatui lines, honouring the
/// current view settings. `display_order` is the order the sidebar
/// would render files in; `file_offsets` is keyed by *input* index so
/// callers always look up `file_offsets[input_index]`.
#[allow(clippy::too_many_arguments)]
fn build_view(
    files: &[ResolvedFile<'_>],
    diffs: &[Diff],
    structurals: &[StructuralDiff],
    outlines: &[Vec<OutlineEntry>],
    display_order: &[usize],
    width: usize,
    theme: &Theme,
    settings: ViewSettings,
) -> DiffView {
    let mut lines = Vec::new();
    let mut file_offsets = vec![0usize; files.len()];

    for (display_idx, &input_idx) in display_order.iter().enumerate() {
        if display_idx > 0 {
            lines.push(Line::from(""));
        }
        file_offsets[input_idx] = lines.len();

        let resolved = &files[input_idx];
        let path = display_path(resolved.file);
        lines.extend(render_tui::render_file_header(path, width, theme));

        if let Some(old_path) = &resolved.file.rename_from {
            lines.push(render_tui::render_rename_header(
                old_path,
                &resolved.file.new_path,
                theme,
            ));
        }

        let diff = &diffs[input_idx];
        let structural = &structurals[input_idx];
        let outline = &outlines[input_idx];

        // Outline filters per-row, so the file-level placeholder gate
        // would prematurely hide files whose public symbols were
        // unchanged. Restrict the gate to Full / Summary, where we
        // really do want to drop the whole file.
        let file_level_gate = !matches!(settings.mode, ViewMode::Outline);
        if file_level_gate && settings.public_only && !file_has_public_change(structural) {
            lines.push(Line::from(""));
            lines.push(Line::styled(
                "  (no changes to public symbols)".to_string(),
                Style::default().fg(rgb_to_color(theme.muted)),
            ));
            continue;
        }

        match settings.mode {
            ViewMode::Full => append_full_hunks(&mut lines, diff, structural, width, theme),
            ViewMode::Outline => {
                append_outline_block(
                    &mut lines,
                    &resolved.after,
                    diff.language(),
                    outline,
                    settings,
                    width,
                    theme,
                );
            }
            ViewMode::Summary => {
                append_summary_block(&mut lines, structural, settings, theme, false, false);
            }
        }
    }

    DiffView {
        lines,
        file_offsets,
    }
}

/// Render each hunk as ratatui lines (full diff view). When a hunk
/// falls inside a symbol that has a structural change, an annotation
/// line is rendered just above the hunk so reviewers see the kind of
/// change without leaving the diff ("~ Modified method `Foo::bar`").
fn append_full_hunks(
    out: &mut Vec<Line<'static>>,
    diff: &Diff,
    structural: &StructuralDiff,
    width: usize,
    theme: &Theme,
) {
    let span_index = build_change_span_index(structural);
    for hunk in diff.hunks() {
        out.push(Line::from(""));
        if let Some(change) = annotate_for_hunk(hunk, &span_index)
            && let Some(line) = structural_annotation_line(change, theme)
        {
            out.push(line);
        }
        out.extend(render_tui::render_hunk(hunk, diff.language(), width, theme));
    }
}

/// Index of new-side line spans → the change that covers them. Built
/// per file so we look up O(spans) per hunk; perfectly fine for the
/// scale of changes any one file usually has.
fn build_change_span_index(structural: &StructuralDiff) -> Vec<(LineSpan, &StructuralChange)> {
    structural
        .changes()
        .iter()
        .filter_map(|c| {
            // Prefer the after-side span; fall back to before when the
            // symbol was removed (so removed-hunks still get a label).
            let span = c
                .after
                .as_ref()
                .map(|s| s.span)
                .or_else(|| c.before.as_ref().map(|s| s.span));
            span.map(|s| (s, c))
        })
        .collect()
}

/// Find the *smallest* span that overlaps the hunk's new-side range.
/// Smallest = most specific (so a method beats its enclosing class).
fn annotate_for_hunk<'a>(
    hunk: &deltoids::Hunk,
    index: &'a [(LineSpan, &'a StructuralChange)],
) -> Option<&'a StructuralChange> {
    let (h_start, h_end) = hunk_new_range(hunk);
    let mut best: Option<(usize, &StructuralChange)> = None;
    for (span, change) in index {
        if span.end < h_start || span.start > h_end {
            continue;
        }
        let width = span.end.saturating_sub(span.start);
        if best.map(|(w, _)| width < w).unwrap_or(true) {
            best = Some((width, change));
        }
    }
    best.map(|(_, c)| c)
}

/// Compute (start, end) on the new side covered by `hunk`. End is
/// inclusive. A pure-deletion hunk that adds zero new lines reports
/// (new_start, new_start) so it still matches symbols at that line.
fn hunk_new_range(hunk: &deltoids::Hunk) -> (usize, usize) {
    let new_count = hunk
        .lines
        .iter()
        .filter(|l| matches!(l.kind, LineKind::Added | LineKind::Context))
        .count();
    let start = hunk.new_start.max(1);
    let end = if new_count == 0 {
        start
    } else {
        start + new_count - 1
    };
    (start, end)
}

/// Render a one-line structural label for the given change. Returns
/// `None` for body-only changes (the regular hunk header already shows
/// the function name; the label would be noise).
fn structural_annotation_line(change: &StructuralChange, theme: &Theme) -> Option<Line<'static>> {
    use deltoids::structural::ChangeKind;
    use ratatui::style::Color;

    if matches!(change.kind, ChangeKind::BodyChanged) {
        return None;
    }
    let bullet = match change.kind {
        ChangeKind::Added => '+',
        ChangeKind::Removed => '-',
        ChangeKind::Renamed => '→',
        _ => '~',
    };
    let color = match change.kind {
        ChangeKind::Added => Color::Green,
        ChangeKind::Removed => Color::Red,
        ChangeKind::Renamed => rgb_to_color(theme.muted),
        _ => Color::Yellow,
    };
    Some(Line::from(vec![
        Span::raw("  "),
        Span::styled(format!("{bullet} "), Style::default().fg(color)),
        Span::styled(
            change.description.clone(),
            Style::default().fg(rgb_to_color(theme.muted)),
        ),
    ]))
}

/// Render the structural-change list for one file. `signatures_only`
/// drops body-only changes; `show_signatures` switches the line text
/// to the raw declaration.
fn append_summary_block(
    out: &mut Vec<Line<'static>>,
    structural: &StructuralDiff,
    settings: ViewSettings,
    theme: &Theme,
    signatures_only: bool,
    show_signatures: bool,
) {
    let opts = SummaryOptions {
        indent: "  ",
        title: false,
        public_only: settings.public_only,
        signatures_only,
        show_signatures,
    };
    let body = format_summary_with(structural, &opts);
    if body.trim().is_empty() {
        out.push(Line::from(""));
        out.push(Line::styled(
            "  (no structural changes)".to_string(),
            Style::default().fg(rgb_to_color(theme.muted)),
        ));
        return;
    }
    out.push(Line::from(""));
    for line in body.lines() {
        out.push(structural_line(line, theme));
    }
}

/// Render the **outline** view as a clean document-symbols panel,
/// inspired by VS Code / Helix outline panels: every class, struct,
/// trait, function, method, type, etc. on its own row, indented by
/// depth via tree branches (`├` `└` `│`), with a kind-icon, the
/// symbol's name, and an italic-toned signature. Each row's
/// background reflects the diff status of that symbol (Added green,
/// Removed red, Modified yellow, Unchanged none).
///
/// Comments, line numbers, and source-code bodies are intentionally
/// omitted — this view is the file's *structure*, not its source.
fn append_outline_block(
    out: &mut Vec<Line<'static>>,
    _new_source: &str,
    _language: Option<Language>,
    outline: &[OutlineEntry],
    settings: ViewSettings,
    width: usize,
    theme: &Theme,
) {
    out.push(Line::from(""));

    let kept: Vec<&OutlineEntry> = outline
        .iter()
        .filter(|e| !settings.public_only || matches!(e.visibility, Visibility::Public))
        .collect();

    if kept.is_empty() {
        let label = if settings.public_only {
            "  (no public symbols)"
        } else {
            "  (no symbols extracted)"
        };
        out.push(Line::styled(
            label.to_string(),
            Style::default().fg(rgb_to_color(theme.muted)),
        ));
        return;
    }

    // Pre-compute, for each kept entry, whether it has a later
    // sibling at the same depth under the same parent. We use this to
    // pick `├` vs `└` for the entry's own branch column, and `│ ` vs
    // `  ` for ancestor columns.
    let has_later_sibling = compute_has_later_sibling(&kept);

    // For ancestor columns we need to know, *per ancestor depth at
    // each row*, whether that ancestor has a later sibling. The
    // simplest correct rule: an ancestor at depth `d` is "still open"
    // (column = `│ `) when there's some later kept entry whose path
    // shares the prefix path[..=d] AND whose depth equals d (the
    // ancestor's siblings continue) OR is greater (we're still inside
    // it).
    //
    // We compute this lazily per row.
    for (i, entry) in kept.iter().enumerate() {
        let line = render_outline_row(i, entry, &kept, &has_later_sibling, settings, width, theme);
        out.push(line);
    }
}

/// For each entry index, true when a later entry has the same parent
/// path AND the same depth — i.e. there's another sibling waiting in
/// the list. Drives the `├ ` vs `└ ` choice for the entry's own column.
fn compute_has_later_sibling(entries: &[&OutlineEntry]) -> Vec<bool> {
    let mut out = vec![false; entries.len()];
    for i in 0..entries.len() {
        let depth = entries[i].depth;
        let parent = parent_path(&entries[i].path);
        for later in &entries[i + 1..] {
            if later.depth < depth {
                // Walked out of this scope; no more siblings.
                break;
            }
            if later.depth == depth && parent_path(&later.path) == parent {
                out[i] = true;
                break;
            }
        }
    }
    out
}

fn parent_path(path: &[String]) -> &[String] {
    if path.is_empty() {
        &[]
    } else {
        &path[..path.len() - 1]
    }
}

/// Determine whether each ancestor depth above `entry` has more
/// siblings to come. Returns a vec of length `entry.depth`; index
/// `d` is `true` when an ancestor at depth `d` has a later sibling
/// (so we draw `│ ` for that column rather than blank).
fn ancestor_open_columns(idx: usize, entries: &[&OutlineEntry]) -> Vec<bool> {
    let entry = entries[idx];
    let mut open = vec![false; entry.depth];
    for (d, slot) in open.iter_mut().enumerate() {
        // An ancestor at depth `d` is "open" iff there's a later
        // entry whose parent path matches `entry.path[..d]` (so we're
        // not the last sibling at that depth).
        let prefix_to_match = &entry.path[..d];
        for later in &entries[idx + 1..] {
            if later.depth < d {
                break;
            }
            if later.depth == d && parent_path(&later.path) == prefix_to_match {
                *slot = true;
                break;
            }
        }
    }
    open
}

#[allow(clippy::too_many_arguments)]
fn render_outline_row(
    idx: usize,
    entry: &OutlineEntry,
    entries: &[&OutlineEntry],
    has_later_sibling: &[bool],
    _settings: ViewSettings,
    width: usize,
    theme: &Theme,
) -> Line<'static> {
    let bg = status_background(entry.status, theme);
    let base = match bg {
        Some(c) => Style::default().bg(c),
        None => Style::default(),
    };
    let muted_fg = rgb_to_color(theme.muted);
    let (icon_glyph, icon_color) = kind_icon(&entry.kind);
    let name = entry
        .path
        .last()
        .cloned()
        .unwrap_or_else(|| "<unnamed>".to_string());

    // ── Tree branches ──────────────────────────────────────────────
    let mut prefix = String::from("  ");
    let open = ancestor_open_columns(idx, entries);
    for d in 0..entry.depth {
        prefix.push_str(if open.get(d).copied().unwrap_or(false) {
            "│ "
        } else {
            "  "
        });
    }
    if entry.depth > 0 {
        prefix.push_str(if has_later_sibling[idx] {
            "├ "
        } else {
            "└ "
        });
    }

    // ── Compose ────────────────────────────────────────────────────
    // Layout: <prefix><icon> <name><visibility-dot>  <signature>
    //         …padding…   <muted description>
    //
    // The name keeps its default foreground colour (no status tint).
    // All status information lives in the muted right-aligned
    // description and the row-wide background tint.
    let visibility_dot = match entry.visibility {
        Visibility::Public => " ●",
        _ => "",
    };
    let description = entry.description();

    // Reserve one column for the scrollbar (drawn over the rightmost
    // inner column by ratatui).
    let scrollbar_col: usize = 1;

    let mut spans: Vec<Span<'static>> = Vec::new();
    spans.push(Span::styled(prefix, base));
    spans.push(Span::styled(format!("{icon_glyph} "), base.fg(icon_color)));
    spans.push(Span::styled(
        name.clone(),
        base.add_modifier(ratatui::style::Modifier::BOLD),
    ));
    if !visibility_dot.is_empty() {
        spans.push(Span::styled(visibility_dot.to_string(), base.fg(muted_fg)));
    }

    // Pad to the right edge (minus scrollbar) and append the muted
    // description there, so every description aligns visually.
    let used: usize = spans.iter().map(|s| s.content.width()).sum();
    let desc_with_gap = if description.is_empty() {
        0
    } else {
        description.width() + 2 // 2-space gutter before the description
    };
    let pad = width
        .saturating_sub(scrollbar_col)
        .saturating_sub(used + desc_with_gap);
    if pad > 0 {
        spans.push(Span::styled(" ".repeat(pad), base));
    }
    if !description.is_empty() {
        spans.push(Span::styled(
            format!("  {description}"),
            base.fg(muted_fg)
                .add_modifier(ratatui::style::Modifier::ITALIC),
        ));
    }

    Line::from(spans)
}

/// Pick a glyph + colour for a symbol kind. Stays terminal-safe with
/// single-character Unicode glyphs that read across most fonts.
fn kind_icon(kind: &deltoids::SymbolKind) -> (&'static str, ratatui::style::Color) {
    use deltoids::SymbolKind as K;
    use ratatui::style::Color;
    match kind {
        K::Function => ("ƒ", Color::Cyan),
        K::Method => ("ƒ", Color::Cyan),
        K::Class => ("◇", Color::Magenta),
        K::Struct => ("◇", Color::Magenta),
        K::Enum => ("≡", Color::Magenta),
        K::Trait => ("◆", Color::LightBlue),
        K::Type => ("τ", Color::LightBlue),
        K::Const => ("●", Color::LightGreen),
        K::Module => ("▣", Color::Blue),
        K::Field => ("•", Color::White),
        K::Macro => ("ℳ", Color::Yellow),
        K::Impl => ("◈", Color::LightMagenta),
        K::Other(_) => ("?", Color::Gray),
    }
}

/// Background colour per status. `None` = no background.
fn status_background(status: OutlineStatus, theme: &Theme) -> Option<ratatui::style::Color> {
    match status {
        OutlineStatus::Unchanged => None,
        OutlineStatus::Added => Some(rgb_to_color(theme.diff_added_bg)),
        OutlineStatus::Removed => Some(rgb_to_color(theme.diff_deleted_bg)),
        OutlineStatus::BodyChanged
        | OutlineStatus::SignatureChanged
        | OutlineStatus::VisibilityChanged
        | OutlineStatus::Modified
        | OutlineStatus::Renamed => Some(rgb_to_color(theme.diff_added_emph_bg)),
    }
}

/// Style a single structural-summary line with colours that reflect
/// the leading bullet (`+` green, `-` red, `→` muted, `~` yellow).
fn structural_line(line: &str, theme: &Theme) -> Line<'static> {
    use ratatui::style::Color;
    let trimmed = line.trim_start_matches(' ');
    let indent_len = line.len() - trimmed.len();
    let indent = &line[..indent_len];
    let mut chars = trimmed.chars();
    let bullet = chars.next().unwrap_or(' ');
    let rest: String = chars.collect();
    let color = match bullet {
        '+' => Color::Green,
        '-' => Color::Red,
        '→' => rgb_to_color(theme.muted),
        _ => Color::Yellow,
    };
    Line::from(vec![
        Span::raw(indent.to_string()),
        Span::styled(format!("{bullet} "), Style::default().fg(color)),
        Span::raw(rest),
    ])
}

// ---------------------------------------------------------------------------
// Scroll state
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus {
    Sidebar,
    Diff,
}

/// What the right pane shows. Cycled with the `v` key. Layered on top
/// of the public-only filter; both can be active at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ViewMode {
    /// Full unified diff with deltoids breadcrumb context.
    Full,
    /// File outline: every class / function / method / type with a
    /// diff-coloured background indicating its status (Unchanged /
    /// Added / Removed / Modified / etc.).
    Outline,
    /// Compact change summary (one line per moved symbol).
    Summary,
}

impl ViewMode {
    fn cycle(self) -> Self {
        match self {
            ViewMode::Full => ViewMode::Outline,
            ViewMode::Outline => ViewMode::Summary,
            ViewMode::Summary => ViewMode::Full,
        }
    }

    fn label(self) -> &'static str {
        match self {
            ViewMode::Full => "full",
            ViewMode::Outline => "outline",
            ViewMode::Summary => "summary",
        }
    }

    fn pane_title(self) -> &'static str {
        match self {
            ViewMode::Full => " [2] Diff ",
            ViewMode::Outline => " [2] Outline ",
            ViewMode::Summary => " [2] Summary ",
        }
    }
}

/// View configuration that affects what `build_view` produces. Cheap
/// to clone; stored on `ViewState` so it survives across resize
/// rebuilds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ViewSettings {
    mode: ViewMode,
    public_only: bool,
}

impl Default for ViewSettings {
    fn default() -> Self {
        Self {
            mode: ViewMode::Full,
            public_only: false,
        }
    }
}

struct ViewState {
    /// Cached diff lines, valid for `cached_width`.
    diff_lines: Vec<Line<'static>>,
    /// Per-file row offsets into `diff_lines`. Indexed by *input* index;
    /// the value is the line in `diff_lines` where that file starts.
    file_offsets: Vec<usize>,
    /// File indices in sidebar (display) order. Cached so resize
    /// rebuilds reuse the same order.
    display_order: Vec<usize>,
    /// The width `diff_lines` was built for; rebuild when the diff pane
    /// resizes.
    cached_width: usize,
    /// Vertical scroll offset (in lines) for the diff pane.
    diff_scroll: usize,
    /// Sidebar state: rows, selection, scroll. Built once at startup.
    sidebar: Sidebar,
    /// Currently-focused pane. Determines where j/k/g/G/PgUp/PgDn go.
    focus: Focus,
    /// View configuration (mode + public-only filter).
    settings: ViewSettings,
    /// Settings the cache was built for. `None` means "never built";
    /// the resize loop rebuilds when this drifts from `settings`.
    cached_settings: Option<ViewSettings>,
}

impl ViewState {
    fn new(
        view: DiffView,
        sidebar: Sidebar,
        display_order: Vec<usize>,
        width: usize,
        settings: ViewSettings,
    ) -> Self {
        Self {
            diff_lines: view.lines,
            file_offsets: view.file_offsets,
            display_order,
            cached_width: width,
            diff_scroll: 0,
            sidebar,
            focus: Focus::Sidebar,
            settings,
            cached_settings: Some(settings),
        }
    }

    /// Window of `diff_lines` that should be visible right now.
    ///
    /// The diff pane is always filtered to whatever the sidebar is
    /// pointing at: a directory header narrows to that subtree's
    /// files, a file row narrows to that single file. Empty diff
    /// (no files at all) falls through to the full slice so the
    /// pane simply renders nothing.
    fn visible_diff_range(&self) -> std::ops::Range<usize> {
        let Some(display_range) = self.sidebar.selection_display_range() else {
            return 0..self.diff_lines.len();
        };
        if display_range.is_empty() || self.display_order.is_empty() {
            return 0..self.diff_lines.len();
        }
        let first_input = self.display_order[display_range.start];
        let start = self.file_offsets[first_input];
        let end = if display_range.end < self.display_order.len() {
            // Stop just before the blank separator that precedes the
            // next file. file_offsets points at the file *header*, so
            // the line immediately above is the separator.
            let next_input = self.display_order[display_range.end];
            self.file_offsets[next_input].saturating_sub(1)
        } else {
            self.diff_lines.len()
        };
        start..end
    }

    /// Maximum scroll offset (an absolute index in `diff_lines`) such
    /// that the viewport still sits inside the current visible range.
    fn max_diff_scroll(&self, viewport: usize) -> usize {
        let range = self.visible_diff_range();
        let span = range.end.saturating_sub(range.start);
        range.start + span.saturating_sub(viewport.max(1))
    }

    /// Lower bound for `diff_scroll` (start of the visible range).
    fn min_diff_scroll(&self) -> usize {
        self.visible_diff_range().start
    }

    fn scroll_diff_by(&mut self, delta: isize, viewport: usize) {
        let min = self.min_diff_scroll() as isize;
        let max = self.max_diff_scroll(viewport) as isize;
        let target = (self.diff_scroll as isize + delta).clamp(min, max.max(min));
        self.diff_scroll = target as usize;
    }

    fn scroll_diff_to_top(&mut self) {
        self.diff_scroll = self.min_diff_scroll();
    }

    fn scroll_diff_to_bottom(&mut self, viewport: usize) {
        self.diff_scroll = self.max_diff_scroll(viewport);
    }

    /// Sync the diff pane's scroll to the file the sidebar is pointing
    /// at. On a file row that's the selected file; on a directory row
    /// it's the first file inside that subtree, so the diff updates as
    /// the user traverses the tree. Scroll is also clamped to the
    /// visible range.
    fn snap_diff_to_selected_file(&mut self, viewport: usize) {
        let Some(file_idx) = self.sidebar.nearest_file_index() else {
            return;
        };
        let Some(&offset) = self.file_offsets.get(file_idx) else {
            return;
        };
        let min = self.min_diff_scroll();
        let max = self.max_diff_scroll(viewport);
        self.diff_scroll = offset.clamp(min, max.max(min));
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AppCommand {
    Continue,
    Quit,
}

fn handle_key(
    state: &mut ViewState,
    key: KeyCode,
    diff_viewport: usize,
    sidebar_viewport: usize,
) -> AppCommand {
    match key {
        KeyCode::Char('q') | KeyCode::Esc => AppCommand::Quit,
        KeyCode::Char('v') => {
            state.settings.mode = state.settings.mode.cycle();
            state.diff_scroll = 0;
            AppCommand::Continue
        }
        KeyCode::Char('p') => {
            state.settings.public_only = !state.settings.public_only;
            state.diff_scroll = 0;
            AppCommand::Continue
        }
        KeyCode::Tab | KeyCode::BackTab => {
            state.focus = match state.focus {
                Focus::Sidebar => Focus::Diff,
                Focus::Diff => Focus::Sidebar,
            };
            AppCommand::Continue
        }
        KeyCode::Char('1') => {
            state.focus = Focus::Sidebar;
            AppCommand::Continue
        }
        KeyCode::Char('2') => {
            state.focus = Focus::Diff;
            AppCommand::Continue
        }
        // Shift+J/K always scroll the diff regardless of focus.
        KeyCode::Char('J') => {
            state.scroll_diff_by(SCROLL_STEP_LARGE as isize, diff_viewport);
            AppCommand::Continue
        }
        KeyCode::Char('K') => {
            state.scroll_diff_by(-(SCROLL_STEP_LARGE as isize), diff_viewport);
            AppCommand::Continue
        }
        _ => handle_navigation_key(state, key, diff_viewport, sidebar_viewport),
    }
}

fn handle_navigation_key(
    state: &mut ViewState,
    key: KeyCode,
    diff_viewport: usize,
    sidebar_viewport: usize,
) -> AppCommand {
    match key {
        KeyCode::Char('j') | KeyCode::Down => match state.focus {
            Focus::Sidebar => {
                state.sidebar.move_down(sidebar_viewport);
                state.snap_diff_to_selected_file(diff_viewport);
                AppCommand::Continue
            }
            Focus::Diff => {
                state.scroll_diff_by(SCROLL_STEP_SMALL as isize, diff_viewport);
                AppCommand::Continue
            }
        },
        KeyCode::Char('k') | KeyCode::Up => match state.focus {
            Focus::Sidebar => {
                state.sidebar.move_up(sidebar_viewport);
                state.snap_diff_to_selected_file(diff_viewport);
                AppCommand::Continue
            }
            Focus::Diff => {
                state.scroll_diff_by(-(SCROLL_STEP_SMALL as isize), diff_viewport);
                AppCommand::Continue
            }
        },
        KeyCode::PageDown | KeyCode::Char(' ') => match state.focus {
            Focus::Sidebar => {
                state.sidebar.page_down(sidebar_viewport);
                state.snap_diff_to_selected_file(diff_viewport);
                AppCommand::Continue
            }
            Focus::Diff => {
                state.scroll_diff_by(diff_viewport.max(1) as isize, diff_viewport);
                AppCommand::Continue
            }
        },
        KeyCode::PageUp => match state.focus {
            Focus::Sidebar => {
                state.sidebar.page_up(sidebar_viewport);
                state.snap_diff_to_selected_file(diff_viewport);
                AppCommand::Continue
            }
            Focus::Diff => {
                state.scroll_diff_by(-(diff_viewport.max(1) as isize), diff_viewport);
                AppCommand::Continue
            }
        },
        KeyCode::Char('g') | KeyCode::Home => match state.focus {
            Focus::Sidebar => {
                state.sidebar.top(sidebar_viewport);
                state.snap_diff_to_selected_file(diff_viewport);
                AppCommand::Continue
            }
            Focus::Diff => {
                state.scroll_diff_to_top();
                AppCommand::Continue
            }
        },
        KeyCode::Char('G') | KeyCode::End => match state.focus {
            Focus::Sidebar => {
                state.sidebar.bottom(sidebar_viewport);
                state.snap_diff_to_selected_file(diff_viewport);
                AppCommand::Continue
            }
            Focus::Diff => {
                state.scroll_diff_to_bottom(diff_viewport);
                AppCommand::Continue
            }
        },
        _ => AppCommand::Continue,
    }
}

// ---------------------------------------------------------------------------
// TUI loop
// ---------------------------------------------------------------------------

/// Compute the sidebar's column width given the terminal width. Returns
/// 0 when the terminal is too narrow to comfortably show the sidebar.
fn sidebar_width(terminal_width: u16) -> u16 {
    if terminal_width < MIN_TERMINAL_WIDTH_FOR_SIDEBAR {
        return 0;
    }
    DEFAULT_SIDEBAR_WIDTH.min(terminal_width / 3)
}

fn run_tui(
    files: &[ResolvedFile<'_>],
    diffs: &[Diff],
    structurals: &[StructuralDiff],
    outlines: &[Vec<OutlineEntry>],
    theme: &Theme,
) -> Result<(), String> {
    let stdout = io::stdout();
    let backend = CrosstermBackend::new(stdout);
    let mut terminal =
        Terminal::new(backend).map_err(|err| format!("failed to create screen: {err}"))?;
    let _session = TerminalSession::enter(&mut terminal)?;

    // Build sidebar from the resolved files plus per-file delta counts.
    let sidebar_files: Vec<SidebarFile<'_>> = files
        .iter()
        .zip(diffs.iter())
        .map(|(f, d)| {
            let (added, deleted) = count_deltas(d);
            SidebarFile {
                file: f.file,
                added,
                deleted,
            }
        })
        .collect();
    let sidebar = Sidebar::build(&sidebar_files, theme);

    // Build the diff view for the initial diff-pane width, then rebuild
    // on resize.
    let initial_total_width = terminal.size().map(|s| s.width).unwrap_or(120);
    let initial_diff_width = diff_pane_width(initial_total_width);

    let display_order = sidebar.display_order();
    let settings = ViewSettings::default();
    let view = build_view(
        files,
        diffs,
        structurals,
        outlines,
        &display_order,
        initial_diff_width,
        theme,
        settings,
    );
    let mut state = ViewState::new(view, sidebar, display_order, initial_diff_width, settings);

    // Snap diff to the sidebar's initial selection. The viewport isn't
    // known until the first draw, so approximate it from terminal
    // height; the next iteration's resize check fixes any mismatch.
    let initial_diff_viewport = terminal
        .size()
        .map(|s| s.height.saturating_sub(1) as usize)
        .unwrap_or(40);
    state.snap_diff_to_selected_file(initial_diff_viewport);

    loop {
        // Draw and capture viewport metrics for the current frame.
        let metrics = terminal
            .draw(|frame| draw(frame, &mut state, theme))
            .map_err(|err| format!("failed to render screen: {err}"))?;
        let total_width = metrics.area.width;
        let total_height = metrics.area.height;
        let diff_width = diff_pane_width(total_width);
        // -1 for the help bar at the bottom, -2 for the pane's top and
        // bottom borders. The result is the number of content rows the
        // pane shows, i.e. the scroll viewport.
        let pane_viewport = total_height.saturating_sub(3) as usize;
        let diff_viewport = pane_viewport;
        let sidebar_viewport = pane_viewport;

        // Rebuild the diff line cache if the diff pane changed width
        // OR the user toggled view settings since the last frame.
        let want_rebuild = (diff_width != state.cached_width && diff_width > 0)
            || state.cached_settings != Some(state.settings);
        if want_rebuild && diff_width > 0 {
            let view = build_view(
                files,
                diffs,
                structurals,
                outlines,
                &state.display_order,
                diff_width,
                theme,
                state.settings,
            );
            state.diff_lines = view.lines;
            state.file_offsets = view.file_offsets;
            state.cached_width = diff_width;
            state.cached_settings = Some(state.settings);
            let min = state.min_diff_scroll();
            let max = state.max_diff_scroll(diff_viewport);
            state.diff_scroll = state.diff_scroll.clamp(min, max.max(min));
        }

        let cmd = read_event(diff_viewport, sidebar_viewport, &mut state)?;
        if cmd == AppCommand::Quit {
            break;
        }
    }

    Ok(())
}

/// Width budget for the diff pane *content* (terminal minus the
/// sidebar pane minus this pane's own two border columns). When no
/// sidebar is shown the diff pane spans the whole terminal, still
/// minus its own two borders.
fn diff_pane_width(terminal_width: u16) -> usize {
    let sw = sidebar_width(terminal_width);
    terminal_width.saturating_sub(sw + 2) as usize
}

fn read_event(
    diff_viewport: usize,
    sidebar_viewport: usize,
    state: &mut ViewState,
) -> Result<AppCommand, String> {
    use std::time::Duration;

    if !event::poll(Duration::from_millis(250))
        .map_err(|err| format!("failed to poll input event: {err}"))?
    {
        return Ok(AppCommand::Continue);
    }
    match event::read().map_err(|err| format!("failed to read input event: {err}"))? {
        Event::Key(key) if key.kind == KeyEventKind::Press => {
            Ok(handle_key(state, key.code, diff_viewport, sidebar_viewport))
        }
        Event::Resize(_, _) => Ok(AppCommand::Continue),
        _ => Ok(AppCommand::Continue),
    }
}

fn draw(frame: &mut ratatui::Frame<'_>, state: &mut ViewState, theme: &Theme) {
    let area = frame.area();

    // Vertical: body | help bar.
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(area);
    let body = root[0];
    let help_area = root[1];

    let sw = sidebar_width(body.width);
    if sw > 0 {
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(sw), Constraint::Min(10)])
            .split(body);
        draw_sidebar(frame, cols[0], state, theme);
        draw_diff(frame, cols[1], state, theme);
    } else {
        draw_diff(frame, body, state, theme);
    }

    draw_help(frame, help_area, state, theme);
}

fn draw_sidebar(frame: &mut ratatui::Frame<'_>, area: Rect, state: &mut ViewState, theme: &Theme) {
    let inner = area.inner(Margin {
        vertical: 1,
        horizontal: 1,
    });
    let viewport = inner.height as usize;
    let inner_width = inner.width as usize;
    let scroll = state.sidebar.scroll();
    let total = state.sidebar.row_count();
    let start = scroll.min(total);
    let end = start.saturating_add(viewport.max(1)).min(total);
    let mut visible: Vec<Line<'static>> = state.sidebar.rows()[start..end].to_vec();

    // Extend the selection background across the full inner pane width
    // so the highlighted row reads as a continuous bar (matching
    // lazygit and edit-tui's `List` widget). Pad against the inner
    // width so the trailing block stops just before the right border.
    if let Some(rel) = state.sidebar.selected().checked_sub(scroll)
        && rel < visible.len()
    {
        pad_selected_row(&mut visible[rel], inner_width, theme);
    }

    let color = pane_border_color(state.focus == Focus::Sidebar, theme);
    let footer = sidebar_footer(state);
    let block = pane_block_with_footer(" [1] Files ", color, footer);
    frame.render_widget(Paragraph::new(visible).block(block), area);

    render_pane_scrollbar(
        frame,
        area,
        total,
        state.sidebar.selected(),
        pane_inner_height(area),
        theme,
    );
}

/// Append a trailing span of `selection_bg`-styled spaces so the row's
/// highlight extends to `width`. No-op when the row is already wider
/// than the pane (ratatui clips overflow).
fn pad_selected_row(line: &mut Line<'static>, width: usize, theme: &Theme) {
    let current: usize = line.spans.iter().map(|s| s.content.width()).sum();
    if current >= width {
        return;
    }
    let pad = width - current;
    line.spans.push(Span::styled(
        " ".repeat(pad),
        Style::default().bg(rgb_to_color(theme.selection_bg)),
    ));
}

fn draw_diff(frame: &mut ratatui::Frame<'_>, area: Rect, state: &mut ViewState, theme: &Theme) {
    let inner = area.inner(Margin {
        vertical: 1,
        horizontal: 1,
    });
    let viewport = inner.height as usize;
    let range = state.visible_diff_range();
    let scroll = state.diff_scroll.clamp(range.start, range.end);
    let end = scroll.saturating_add(viewport.max(1)).min(range.end);
    let visible: Vec<Line<'static>> = state.diff_lines[scroll..end].to_vec();

    let color = pane_border_color(state.focus == Focus::Diff, theme);
    let footer = diff_footer(state);
    let block = pane_block_with_footer(state.settings.mode.pane_title(), color, footer);
    frame.render_widget(Paragraph::new(visible).block(block), area);

    // Vertical scrollbar reflects the *visible range*, not the full
    // diff: when the sidebar is on a directory the scrollbar tracks
    // progress through that subtree's files.
    let span = range.end.saturating_sub(range.start);
    let position = scroll.saturating_sub(range.start);
    render_pane_scrollbar(frame, area, span, position, pane_inner_height(area), theme);
}

fn draw_help(frame: &mut ratatui::Frame<'_>, area: Rect, state: &ViewState, theme: &Theme) {
    let mut text = String::from(
        "Tab/1/2 focus  j/k move  Shift+J/K scroll  g/G top/bottom  v view  p public  q quit  ",
    );
    text.push_str(&format!("[view: {}", state.settings.mode.label()));
    if state.settings.public_only {
        text.push_str(", public-only");
    }
    text.push(']');
    let p = Paragraph::new(text).style(Style::default().fg(rgb_to_color(theme.muted)));
    frame.render_widget(p, area);
}

/// Build the sidebar pane's bottom-right footer: file/dir position
/// among all files plus the aggregate `+N -N` line counts.
///
/// Returns `None` when there are no files to display.
fn sidebar_footer(state: &ViewState) -> Option<String> {
    let total = state.display_order.len();
    if total == 0 {
        return None;
    }
    let selected_input = state.sidebar.nearest_file_index()?;
    let pos = state
        .display_order
        .iter()
        .position(|&i| i == selected_input)
        .map(|p| p + 1)
        .unwrap_or(0);
    let label = if state.sidebar.selected_is_dir() {
        "dir"
    } else {
        "file"
    };
    let totals = state.sidebar.totals();
    let mut s = format!(" {label} {pos} of {total}");
    if totals.added > 0 || totals.deleted > 0 {
        s.push_str("  ");
        if totals.added > 0 {
            s.push_str(&format!("+{}", totals.added));
            if totals.deleted > 0 {
                s.push(' ');
            }
        }
        if totals.deleted > 0 {
            s.push_str(&format!("-{}", totals.deleted));
        }
    }
    s.push(' ');
    Some(s)
}

/// Build the diff pane's bottom-right footer: `" line X of Y "` for
/// the current scroll position within the visible range, or `None`
/// when the pane is empty.
fn diff_footer(state: &ViewState) -> Option<String> {
    let range = state.visible_diff_range();
    let span = range.end.saturating_sub(range.start);
    if span == 0 {
        return None;
    }
    let pos = state
        .diff_scroll
        .saturating_sub(range.start)
        .min(span.saturating_sub(1))
        + 1;
    Some(format!(" line {pos} of {span} "))
}

struct TerminalSession;

impl TerminalSession {
    fn enter<B: ratatui::backend::Backend>(terminal: &mut Terminal<B>) -> Result<Self, String> {
        crossterm::terminal::enable_raw_mode()
            .map_err(|err| format!("failed to enable raw mode: {err}"))?;
        crossterm::execute!(
            io::stdout(),
            crossterm::terminal::EnterAlternateScreen,
            crossterm::cursor::Hide
        )
        .map_err(|err| format!("failed to enter screen: {err}"))?;
        terminal
            .clear()
            .map_err(|err| format!("failed to clear screen: {err}"))?;
        Ok(Self)
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        let _ = crossterm::terminal::disable_raw_mode();
        let _ = crossterm::execute!(
            io::stdout(),
            crossterm::terminal::LeaveAlternateScreen,
            crossterm::cursor::Show
        );
        let _ = io::stdout().flush();
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn theme() -> Theme {
        Theme::default()
    }

    /// Build a `FileDiff` with the given path. The `hunks` field is left
    /// empty: `build_view` runs `Diff::compute` against the supplied
    /// before/after text, so the parsed hunks aren't read.
    fn file_diff(path: &str) -> FileDiff {
        FileDiff {
            preamble: Vec::new(),
            old_path: path.to_string(),
            new_path: path.to_string(),
            rename_from: None,
            old_hash: None,
            new_hash: None,
            hunks: Vec::new(),
        }
    }

    /// Concatenate the visible text of a `Line<'static>`.
    fn line_text(line: &Line<'static>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    fn make_state(files: &[ResolvedFile<'_>]) -> ViewState {
        let diffs = precompute_diffs(files);
        let structurals = precompute_structurals(files);
        let sidebar_files: Vec<SidebarFile<'_>> = files
            .iter()
            .zip(diffs.iter())
            .map(|(f, d)| {
                let (added, deleted) = count_deltas(d);
                SidebarFile {
                    file: f.file,
                    added,
                    deleted,
                }
            })
            .collect();
        let sidebar = Sidebar::build_with_icons(&sidebar_files, &theme(), sidebar::IconMode::Off);
        let display_order = sidebar.display_order();
        let settings = ViewSettings::default();
        let outlines = precompute_outlines(files);
        let view = build_view(
            files,
            &diffs,
            &structurals,
            &outlines,
            &display_order,
            80,
            &theme(),
            settings,
        );
        ViewState::new(view, sidebar, display_order, 80, settings)
    }

    #[test]
    fn build_view_emits_file_header_and_hunk_for_one_file() {
        let f = file_diff("foo.txt");
        let resolved = vec![ResolvedFile {
            file: &f,
            before: "hello\n".to_string(),
            after: "world\n".to_string(),
        }];
        let diffs = precompute_diffs(&resolved);
        let view = build_view(
            &resolved,
            &diffs,
            &precompute_structurals(&resolved),
            &precompute_outlines(&resolved),
            &[0],
            80,
            &theme(),
            ViewSettings::default(),
        );
        let texts: Vec<String> = view.lines.iter().map(line_text).collect();

        assert!(
            texts.iter().any(|t| t == "foo.txt"),
            "expected file header, got: {texts:#?}"
        );
        assert!(
            texts.iter().any(|t| t.contains("hello")),
            "expected removed line in: {texts:#?}"
        );
        assert!(
            texts.iter().any(|t| t.contains("world")),
            "expected added line in: {texts:#?}"
        );
    }

    #[test]
    fn build_view_records_one_offset_per_file() {
        let a = file_diff("a.txt");
        let b = file_diff("b.txt");
        let resolved = vec![
            ResolvedFile {
                file: &a,
                before: "a1\n".to_string(),
                after: "a2\n".to_string(),
            },
            ResolvedFile {
                file: &b,
                before: "b1\n".to_string(),
                after: "b2\n".to_string(),
            },
        ];
        let diffs = precompute_diffs(&resolved);
        let view = build_view(
            &resolved,
            &diffs,
            &precompute_structurals(&resolved),
            &precompute_outlines(&resolved),
            &[0, 1],
            80,
            &theme(),
            ViewSettings::default(),
        );
        assert_eq!(view.file_offsets.len(), 2);
        // First offset is 0 (no leading blank).
        assert_eq!(view.file_offsets[0], 0);
        // Second offset points at b's header line.
        let second = view.file_offsets[1];
        let header_text = line_text(&view.lines[second]);
        assert_eq!(header_text, "b.txt");
    }

    #[test]
    fn build_view_renders_in_display_order() {
        // Files supplied in input order [a, b], display order [b, a].
        // Output's first file header must be b's; offsets keyed by
        // input index.
        let a = file_diff("a.txt");
        let b = file_diff("b.txt");
        let resolved = vec![
            ResolvedFile {
                file: &a,
                before: "a1\n".to_string(),
                after: "a2\n".to_string(),
            },
            ResolvedFile {
                file: &b,
                before: "b1\n".to_string(),
                after: "b2\n".to_string(),
            },
        ];
        let diffs = precompute_diffs(&resolved);
        let view = build_view(
            &resolved,
            &diffs,
            &precompute_structurals(&resolved),
            &precompute_outlines(&resolved),
            &[1, 0],
            80,
            &theme(),
            ViewSettings::default(),
        );
        assert_eq!(line_text(&view.lines[0]), "b.txt");
        assert_eq!(view.file_offsets[1], 0);
        assert!(view.file_offsets[0] > 0);
    }

    #[test]
    fn build_view_includes_rename_header_when_renamed() {
        let mut f = file_diff("new.txt");
        f.old_path = "old.txt".to_string();
        f.rename_from = Some("old.txt".to_string());
        let resolved = vec![ResolvedFile {
            file: &f,
            before: "x\n".to_string(),
            after: "y\n".to_string(),
        }];
        let diffs = precompute_diffs(&resolved);
        let view = build_view(
            &resolved,
            &diffs,
            &precompute_structurals(&resolved),
            &precompute_outlines(&resolved),
            &[0],
            80,
            &theme(),
            ViewSettings::default(),
        );
        let combined: String = view
            .lines
            .iter()
            .map(line_text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            combined.contains("renamed:")
                && combined.contains("old.txt")
                && combined.contains("new.txt"),
            "missing rename header in: {combined}"
        );
    }

    #[test]
    fn count_deltas_counts_added_and_removed() {
        let f = file_diff("a.txt");
        let resolved = vec![ResolvedFile {
            file: &f,
            before: "old1\nold2\nshared\n".to_string(),
            after: "new1\nshared\nnew2\n".to_string(),
        }];
        let diffs = precompute_diffs(&resolved);
        let (added, deleted) = count_deltas(&diffs[0]);
        assert!(added > 0, "expected adds");
        assert!(deleted > 0, "expected dels");
    }

    #[test]
    fn handle_key_q_quits() {
        let f = file_diff("a.txt");
        let resolved = vec![ResolvedFile {
            file: &f,
            before: "a\n".to_string(),
            after: "b\n".to_string(),
        }];
        let mut state = make_state(&resolved);
        assert_eq!(
            handle_key(&mut state, KeyCode::Char('q'), 4, 4),
            AppCommand::Quit
        );
    }

    #[test]
    fn handle_key_tab_toggles_focus() {
        let f = file_diff("a.txt");
        let resolved = vec![ResolvedFile {
            file: &f,
            before: "a\n".to_string(),
            after: "b\n".to_string(),
        }];
        let mut state = make_state(&resolved);
        assert_eq!(state.focus, Focus::Sidebar);
        handle_key(&mut state, KeyCode::Tab, 4, 4);
        assert_eq!(state.focus, Focus::Diff);
        handle_key(&mut state, KeyCode::Tab, 4, 4);
        assert_eq!(state.focus, Focus::Sidebar);
    }

    #[test]
    fn handle_key_j_in_diff_focus_scrolls_diff() {
        // Build a diff with enough lines to scroll.
        let f = file_diff("a.txt");
        let resolved = vec![ResolvedFile {
            file: &f,
            before: (0..50).map(|i| format!("line {i}\n")).collect::<String>(),
            after: (0..50).map(|i| format!("line {i}!\n")).collect::<String>(),
        }];
        let mut state = make_state(&resolved);
        state.focus = Focus::Diff;
        handle_key(&mut state, KeyCode::Char('j'), 4, 4);
        assert_eq!(state.diff_scroll, 1);
    }

    #[test]
    fn handle_key_j_in_sidebar_focus_moves_sidebar_and_snaps_diff() {
        let a = file_diff("a.txt");
        let b = file_diff("b.txt");
        let resolved = vec![
            ResolvedFile {
                file: &a,
                before: "a1\n".to_string(),
                after: "a2\n".to_string(),
            },
            ResolvedFile {
                file: &b,
                before: "b1\n".to_string(),
                after: "b2\n".to_string(),
            },
        ];
        let mut state = make_state(&resolved);
        assert_eq!(state.focus, Focus::Sidebar);
        // Initial selection is file 0, diff_scroll 0.
        assert_eq!(state.sidebar.selected_file_index(), Some(0));
        assert_eq!(state.diff_scroll, 0);

        // Use a viewport smaller than the rendered diff so snapping
        // actually moves the scroll offset (otherwise it clamps to 0).
        handle_key(&mut state, KeyCode::Char('j'), 2, 4);
        // Sidebar should now be on file 1.
        assert_eq!(state.sidebar.selected_file_index(), Some(1));
        // Diff scroll should be at file 1's offset.
        assert_eq!(state.diff_scroll, state.file_offsets[1]);
    }

    #[test]
    fn dir_filter_excludes_files_outside_subtree() {
        // Three files under three different dirs. Each file's diff has
        // a unique marker line so we can assert exactly which files are
        // visible at any time.
        let a = file_diff("alpha/a.rs");
        let b = file_diff("beta/b.rs");
        let c = file_diff("gamma/c.rs");
        let resolved = vec![
            ResolvedFile {
                file: &a,
                before: "old_alpha\n".to_string(),
                after: "new_alpha\n".to_string(),
            },
            ResolvedFile {
                file: &b,
                before: "old_beta\n".to_string(),
                after: "new_beta\n".to_string(),
            },
            ResolvedFile {
                file: &c,
                before: "old_gamma\n".to_string(),
                after: "new_gamma\n".to_string(),
            },
        ];
        let mut state = make_state(&resolved);
        // Walk to the `beta/` dir header. Tree order: alpha/ (dir 0),
        // alpha/a.rs (file 0), beta/ (dir 1), beta/b.rs (file 1),
        // gamma/ (dir 2), gamma/c.rs (file 2).
        // Initial selection is on file 0 (alpha/a.rs at row 1).
        // Step down to row 2 = beta/.
        state.sidebar.move_down(20);
        assert!(state.sidebar.selected_is_dir());

        let range = state.visible_diff_range();
        let visible_text: String = state.diff_lines[range.clone()]
            .iter()
            .map(line_text)
            .collect::<Vec<_>>()
            .join("\n");

        // Only beta/b.rs's content must be inside the range.
        assert!(
            visible_text.contains("beta/b.rs")
                && visible_text.contains("old_beta")
                && visible_text.contains("new_beta"),
            "beta content missing from filtered range: {visible_text:?}"
        );
        assert!(
            !visible_text.contains("alpha/a.rs") && !visible_text.contains("old_alpha"),
            "alpha leaked into beta filter: {visible_text:?}"
        );
        assert!(
            !visible_text.contains("gamma/c.rs") && !visible_text.contains("old_gamma"),
            "gamma leaked into beta filter: {visible_text:?}"
        );

        // Move to a file row — visible range narrows to that single file.
        state.sidebar.move_down(20); // file row inside beta/
        assert!(!state.sidebar.selected_is_dir());
        let file_range = state.visible_diff_range();
        let file_text: String = state.diff_lines[file_range]
            .iter()
            .map(line_text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            file_text.contains("beta/b.rs") && file_text.contains("new_beta"),
            "beta file content missing from filtered range: {file_text:?}"
        );
        assert!(
            !file_text.contains("alpha/a.rs") && !file_text.contains("gamma/c.rs"),
            "siblings leaked into single-file filter: {file_text:?}"
        );
    }

    #[test]
    fn visible_diff_range_narrows_to_subtree_on_dir_selection() {
        // Two files in different dirs: src/a.rs and other/b.rs. The
        // sidebar tree puts each under its own dir header. Selecting
        // src/ should restrict visible_diff_range to just src/a.rs's
        // lines; selecting other/ should restrict to other/b.rs.
        let a = file_diff("src/a.rs");
        let b = file_diff("other/b.rs");
        let resolved = vec![
            ResolvedFile {
                file: &a,
                before: "a1\n".to_string(),
                after: "a2\n".to_string(),
            },
            ResolvedFile {
                file: &b,
                before: "b1\n".to_string(),
                after: "b2\n".to_string(),
            },
        ];
        let mut state = make_state(&resolved);

        // Initial selection is on a file: visible range is exactly
        // that file (single-element subset of the full diff).
        let file_range = state.visible_diff_range();
        assert!(
            file_range.end - file_range.start < state.diff_lines.len(),
            "file selection should narrow to a single file's slice"
        );
        let file_first_line = line_text(&state.diff_lines[file_range.start]);
        assert!(
            file_first_line == "src/a.rs" || file_first_line == "other/b.rs",
            "expected a file header at start, got {file_first_line:?}"
        );

        // Move up onto the dir header above the first file (other/ is
        // first alphabetically among the directory rows).
        state.sidebar.top(20);
        assert!(state.sidebar.selected_is_dir());
        let narrowed = state.visible_diff_range();
        // The range must be strictly smaller than the full diff.
        assert!(
            narrowed.end - narrowed.start < state.diff_lines.len(),
            "expected subtree range to be narrower than full diff"
        );
        // The very first visible line should be the file header for
        // whichever file the dir contains.
        let first_line = line_text(&state.diff_lines[narrowed.start]);
        assert!(
            first_line == "src/a.rs" || first_line == "other/b.rs",
            "expected dir's file header at start, got {first_line:?}"
        );
    }

    #[test]
    fn handle_key_capital_j_scrolls_diff_in_sidebar_focus() {
        let f = file_diff("a.txt");
        let resolved = vec![ResolvedFile {
            file: &f,
            before: (0..50).map(|i| format!("line {i}\n")).collect::<String>(),
            after: (0..50).map(|i| format!("line {i}!\n")).collect::<String>(),
        }];
        let mut state = make_state(&resolved);
        // Stay in Sidebar focus; Shift+J should still scroll the diff.
        assert_eq!(state.focus, Focus::Sidebar);
        handle_key(&mut state, KeyCode::Char('J'), 4, 4);
        assert_eq!(state.diff_scroll, SCROLL_STEP_LARGE);
    }

    #[test]
    fn missing_blob_propagates_error() {
        // Forge a diff whose old blob hash is non-null and unresolvable.
        let diff = "diff --git a/foo.txt b/foo.txt\n\
                    index deadbeefdeadbeefdeadbeefdeadbeefdeadbeef..0000000000000000000000000000000000000000 100644\n\
                    --- a/foo.txt\n\
                    +++ /dev/null\n\
                    @@ -1 +0,0 @@\n\
                    -gone\n";
        let parsed = GitDiff::parse(diff);
        let Err(err) = resolve(&parsed, None) else {
            panic!("resolve should fail on missing blob");
        };
        assert!(err.contains("missing index blob"), "got: {err}");
        assert!(err.contains("foo.txt"), "got: {err}");
    }

    #[test]
    fn sidebar_width_hides_when_terminal_is_narrow() {
        assert_eq!(sidebar_width(60), 0);
    }

    #[test]
    fn sidebar_width_caps_at_third_of_terminal() {
        // Plenty wide → use the default width.
        assert_eq!(sidebar_width(200), DEFAULT_SIDEBAR_WIDTH);
        // Narrower terminal → capped at third.
        assert_eq!(sidebar_width(90), 30);
    }

    // ---------------------------------------------------------------
    // View-mode + public-only filter
    // ---------------------------------------------------------------

    #[test]
    fn view_mode_cycle_full_outline_summary() {
        assert_eq!(ViewMode::Full.cycle(), ViewMode::Outline);
        assert_eq!(ViewMode::Outline.cycle(), ViewMode::Summary);
        assert_eq!(ViewMode::Summary.cycle(), ViewMode::Full);
    }

    #[test]
    fn pressing_v_cycles_the_view_mode() {
        let f = file_diff("a.rs");
        let resolved = vec![ResolvedFile {
            file: &f,
            before: "fn x() {}\n".to_string(),
            after: "fn y() {}\n".to_string(),
        }];
        let mut state = make_state(&resolved);
        assert_eq!(state.settings.mode, ViewMode::Full);
        handle_key(&mut state, KeyCode::Char('v'), 4, 4);
        assert_eq!(state.settings.mode, ViewMode::Outline);
        handle_key(&mut state, KeyCode::Char('v'), 4, 4);
        assert_eq!(state.settings.mode, ViewMode::Summary);
        handle_key(&mut state, KeyCode::Char('v'), 4, 4);
        assert_eq!(state.settings.mode, ViewMode::Full);
    }

    #[test]
    fn pressing_p_toggles_public_only() {
        let f = file_diff("a.rs");
        let resolved = vec![ResolvedFile {
            file: &f,
            before: "fn x() {}\n".to_string(),
            after: "fn y() {}\n".to_string(),
        }];
        let mut state = make_state(&resolved);
        assert!(!state.settings.public_only);
        handle_key(&mut state, KeyCode::Char('p'), 4, 4);
        assert!(state.settings.public_only);
        handle_key(&mut state, KeyCode::Char('p'), 4, 4);
        assert!(!state.settings.public_only);
    }

    #[test]
    fn build_view_summary_mode_renders_structural_lines() {
        let f = file_diff("a.rs");
        let resolved = vec![ResolvedFile {
            file: &f,
            before: "fn alpha() {}\n".to_string(),
            after: "fn alpha() {}\nfn beta() {}\n".to_string(),
        }];
        let diffs = precompute_diffs(&resolved);
        let structurals = precompute_structurals(&resolved);
        let outlines = precompute_outlines(&resolved);
        let settings = ViewSettings {
            mode: ViewMode::Summary,
            public_only: false,
        };
        let view = build_view(
            &resolved,
            &diffs,
            &structurals,
            &outlines,
            &[0],
            80,
            &theme(),
            settings,
        );
        let combined: String = view
            .lines
            .iter()
            .map(line_text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            combined.contains("Added function `beta`"),
            "got:\n{combined}"
        );
    }

    #[test]
    fn full_view_annotates_added_function_above_its_hunk() {
        // The added function `brand_new` should produce a structural
        // annotation line above the hunk that adds it.
        let f = file_diff("a.rs");
        let resolved = vec![ResolvedFile {
            file: &f,
            before: "fn helper() {}\n".to_string(),
            after: "fn helper() {}\n\npub fn brand_new() -> i32 { 1 }\n".to_string(),
        }];
        let diffs = precompute_diffs(&resolved);
        let structurals = precompute_structurals(&resolved);
        let outlines = precompute_outlines(&resolved);
        let view = build_view(
            &resolved,
            &diffs,
            &structurals,
            &outlines,
            &[0],
            80,
            &theme(),
            ViewSettings::default(),
        );
        let combined: String = view
            .lines
            .iter()
            .map(line_text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            combined.contains("+ Added function `brand_new` (public)"),
            "got:\n{combined}"
        );
    }

    #[test]
    fn public_only_renders_placeholder_for_private_only_files() {
        let f = file_diff("a.rs");
        let resolved = vec![ResolvedFile {
            file: &f,
            before: "fn priv_a() {}\n".to_string(),
            after: "fn priv_a() { let x = 1; }\n".to_string(),
        }];
        let diffs = precompute_diffs(&resolved);
        let structurals = precompute_structurals(&resolved);
        let outlines = precompute_outlines(&resolved);
        let settings = ViewSettings {
            mode: ViewMode::Summary,
            public_only: true,
        };
        let view = build_view(
            &resolved,
            &diffs,
            &structurals,
            &outlines,
            &[0],
            80,
            &theme(),
            settings,
        );
        let combined: String = view
            .lines
            .iter()
            .map(line_text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            combined.contains("no changes to public symbols"),
            "got:\n{combined}"
        );
    }

    #[test]
    fn outline_row_shows_added_description_in_muted_color() {
        let f = file_diff("a.rs");
        let resolved = vec![ResolvedFile {
            file: &f,
            before: "fn helper() {}\n".to_string(),
            after: "fn helper() {}\nfn brand_new() {}\n".to_string(),
        }];
        let diffs = precompute_diffs(&resolved);
        let structurals = precompute_structurals(&resolved);
        let outlines = precompute_outlines(&resolved);
        let settings = ViewSettings {
            mode: ViewMode::Outline,
            public_only: false,
        };
        let view = build_view(
            &resolved,
            &diffs,
            &structurals,
            &outlines,
            &[0],
            120,
            &theme(),
            settings,
        );
        let combined: String = view
            .lines
            .iter()
            .map(line_text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(combined.contains("brand_new"), "got:\n{combined}");
        assert!(
            combined.contains("added"),
            "description `added` missing: {combined}"
        );
    }

    #[test]
    fn outline_view_lists_every_symbol_with_diff_status() {
        // Old has `helper` and `legacy`. New keeps `helper`, drops
        // `legacy`, and adds `brand_new`. The outline should list all
        // three with their statuses.
        let f = file_diff("a.rs");
        let resolved = vec![ResolvedFile {
            file: &f,
            before: "fn helper() {}\nfn legacy() {}\n".to_string(),
            after: "fn helper() {}\npub fn brand_new() -> i32 { 1 }\n".to_string(),
        }];
        let diffs = precompute_diffs(&resolved);
        let structurals = precompute_structurals(&resolved);
        let outlines = precompute_outlines(&resolved);
        let settings = ViewSettings {
            mode: ViewMode::Outline,
            public_only: false,
        };
        let view = build_view(
            &resolved,
            &diffs,
            &structurals,
            &outlines,
            &[0],
            80,
            &theme(),
            settings,
        );
        let combined: String = view
            .lines
            .iter()
            .map(line_text)
            .collect::<Vec<_>>()
            .join("\n");

        assert!(combined.contains("helper"), "missing unchanged: {combined}");
        assert!(combined.contains("brand_new"), "missing added: {combined}");
        assert!(combined.contains("legacy"), "missing removed: {combined}");
        // Status descriptions should appear, in muted text.
        assert!(
            combined.contains("added"),
            "no `added` description: {combined}"
        );
        assert!(
            combined.contains("removed"),
            "no `removed` description: {combined}"
        );
    }

    #[test]
    fn outline_view_indents_methods_under_their_class() {
        let f = file_diff("a.py");
        let resolved = vec![ResolvedFile {
            file: &f,
            before: "class Foo:\n    def one(self):\n        pass\n".to_string(),
            after: "\
class Foo:\n    def one(self):\n        pass\n    def two(self):\n        pass\n"
                .to_string(),
        }];
        let diffs = precompute_diffs(&resolved);
        let structurals = precompute_structurals(&resolved);
        let outlines = precompute_outlines(&resolved);
        let settings = ViewSettings {
            mode: ViewMode::Outline,
            public_only: false,
        };
        let view = build_view(
            &resolved,
            &diffs,
            &structurals,
            &outlines,
            &[0],
            80,
            &theme(),
            settings,
        );
        // Document-symbols layout: `Foo` shows up at a shallower
        // tree column than its method `two`, because methods nest
        // under their class.
        let foo_col = view
            .lines
            .iter()
            .map(line_text)
            .find_map(|t| t.find(" Foo"))
            .expect("Foo row");
        let two_col = view
            .lines
            .iter()
            .map(line_text)
            .find_map(|t| t.find(" two"))
            .expect("two row");
        assert!(two_col > foo_col, "two col {two_col} <= Foo col {foo_col}",);
    }

    #[test]
    fn outline_view_filters_to_public_when_p_is_on() {
        let f = file_diff("a.rs");
        let resolved = vec![ResolvedFile {
            file: &f,
            before: "fn priv_only() {}\npub fn keep() {}\n".to_string(),
            after: "fn priv_only() { let x = 1; }\npub fn keep() {}\n".to_string(),
        }];
        let diffs = precompute_diffs(&resolved);
        let structurals = precompute_structurals(&resolved);
        let outlines = precompute_outlines(&resolved);
        let settings = ViewSettings {
            mode: ViewMode::Outline,
            public_only: true,
        };
        let view = build_view(
            &resolved,
            &diffs,
            &structurals,
            &outlines,
            &[0],
            80,
            &theme(),
            settings,
        );
        let combined: String = view
            .lines
            .iter()
            .map(line_text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            !combined.contains("priv_only"),
            "private symbol leaked: {combined}"
        );
        assert!(combined.contains("keep"), "missing public: {combined}");
    }
}

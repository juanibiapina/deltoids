//! The reword popup: a commit's message, edited inside the TUI.
//!
//! `r` on a commit opens it over the main view, prefilled with the
//! commit's message. `Enter` saves through [`Vcs::reword`], `Shift+Enter`
//! adds a line, `Esc` cancels, and `Ctrl+g` hands the message to
//! `$EDITOR` and brings the result back. Every other key goes to the
//! textarea. A failed save keeps the text and shows the error.
//!
//! A terminal reports `Shift+Enter` apart from `Enter` only with the
//! kitty keyboard protocol, which [`super::super::terminal`] asks for.

use std::fs;
use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Margin, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use ratatui_textarea::{CursorMove, TextArea};

use deltoids::render_tui::{pane_block_with_title_line, pane_border_color, rgb_to_color};
use deltoids::{Theme, git};

use super::super::comment_view::centered_columns;
use super::super::panel::panel_title;
use super::super::text::wrap_text;
use super::super::vcs::Vcs;

/// The popup's keys, drawn in its bottom border.
const KEYS: &str = "Enter save · Shift+Enter newline · Ctrl+g $EDITOR · Esc cancel";
/// Room for 72-column lines, borders, and padding.
const WIDTH: u16 = 80;
const MIN_HEIGHT: u16 = 8;

pub(super) struct RewordPopup {
    /// The commit selected when `r` was pressed.
    commit: git::Commit,
    editor: TextArea<'static>,
    /// Why the last save or `$EDITOR` run failed, until the next key.
    error: Option<String>,
}

/// What a key did to the popup.
pub(super) enum RewordKey {
    /// Keep the popup open.
    Stay,
    /// Close it. `saved` when a reword was written.
    Close { saved: bool },
    /// Run `$EDITOR` on this file, then call [`RewordPopup::report_edit`].
    Edit(PathBuf),
}

impl RewordPopup {
    pub(super) fn open(commit: &git::Commit) -> Self {
        let mut popup = Self {
            commit: commit.clone(),
            editor: TextArea::default(),
            error: None,
        };
        popup.set_text(commit.message.trim_end());
        popup
    }

    /// Edit `text`, with the cursor at the end of its first line.
    fn set_text(&mut self, text: &str) {
        let mut editor = TextArea::new(text.lines().map(str::to_string).collect());
        editor.set_cursor_line_style(Style::default());
        editor.move_cursor(CursorMove::End);
        self.editor = editor;
    }

    fn text(&self) -> String {
        self.editor.lines().join("\n")
    }

    pub(super) fn handle_key(&mut self, key: KeyEvent, vcs: &Vcs) -> RewordKey {
        self.error = None;
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => return RewordKey::Close { saved: false },
            KeyCode::Enter if key.modifiers == KeyModifiers::SHIFT => self.editor.insert_newline(),
            KeyCode::Enter if key.modifiers.is_empty() => return self.save(vcs),
            // The textarea adds a line on any `Enter` and on `Ctrl+m`;
            // `Shift+Enter` is the only key that does here.
            KeyCode::Enter => {}
            KeyCode::Char('m') if ctrl => {}
            KeyCode::Char('g') if ctrl => return self.edit(),
            _ => {
                self.editor.input(key);
            }
        }
        RewordKey::Stay
    }

    /// Write the message, unless it is empty or unchanged: a reword with
    /// the same message still rewrites the commits above it.
    fn save(&mut self, vcs: &Vcs) -> RewordKey {
        let text = self.text();
        let message = text.trim_end();
        if message.trim().is_empty() {
            self.error = Some("The message is empty".to_string());
            return RewordKey::Stay;
        }
        if message == self.commit.message.trim_end() {
            return RewordKey::Close { saved: false };
        }
        match vcs.reword(&self.commit.id, message) {
            Ok(()) => RewordKey::Close { saved: true },
            Err(err) => {
                self.error = Some(err);
                RewordKey::Stay
            }
        }
    }

    /// The file `$EDITOR` edits, outside the repository. Editors know
    /// the name `COMMIT_EDITMSG` as a commit message. The pid and the
    /// short id keep two deltoids apart.
    fn message_file(&self) -> PathBuf {
        let dir = format!("deltoids-{}-{}", std::process::id(), self.commit.short_id);
        std::env::temp_dir().join(dir).join("COMMIT_EDITMSG")
    }

    /// `Ctrl+g`: write the message for `$EDITOR`.
    fn edit(&mut self) -> RewordKey {
        let path = self.message_file();
        let written = path
            .parent()
            .map_or(Ok(()), fs::create_dir_all)
            .and_then(|()| fs::write(&path, format!("{}\n", self.text())));
        match written {
            Ok(()) => RewordKey::Edit(path),
            Err(err) => {
                self.error = Some(format!("Could not write the message: {err}"));
                RewordKey::Stay
            }
        }
    }

    /// The outcome of the `$EDITOR` run [`RewordKey::Edit`] asked for:
    /// on success the file's text replaces the popup's, else the popup
    /// keeps its text and shows the error.
    pub(super) fn report_edit(&mut self, result: Result<(), String>) {
        let path = self.message_file();
        let text = result.and_then(|()| {
            fs::read_to_string(&path).map_err(|err| format!("Could not read the message: {err}"))
        });
        match text {
            Ok(text) => self.set_text(text.trim_end()),
            Err(err) => self.error = Some(err),
        }
        if let Some(dir) = path.parent() {
            let _ = fs::remove_dir_all(dir);
        }
    }

    /// Draw the popup centered over `area`, as tall as the message plus
    /// an error row.
    pub(super) fn draw(&mut self, frame: &mut Frame<'_>, area: Rect, theme: &Theme) {
        let lines = u16::try_from(self.editor.lines().len()).unwrap_or(u16::MAX);
        let height = lines.saturating_add(3).max(MIN_HEIGHT);
        let popup = centered_columns(area, WIDTH, height);
        let title = panel_title(&format!("Reword {}", self.commit.short_id));
        let footer = Line::from(Span::styled(
            format!(" {KEYS} "),
            Style::default().fg(rgb_to_color(theme.muted)),
        ));
        let block = pane_block_with_title_line(title, pane_border_color(true, theme), Some(footer));
        let inner = block.inner(popup).inner(Margin {
            vertical: 0,
            horizontal: 1,
        });
        frame.render_widget(Clear, popup);
        frame.render_widget(block, popup);
        let error = self
            .error
            .as_deref()
            .map_or_else(Vec::new, |err| wrap_text(err, inner.width as usize));
        // The message keeps at least one row; the error takes the rest.
        let error_rows = u16::try_from(error.len()).unwrap_or(u16::MAX).max(1);
        let [text_area, error_area] =
            Layout::vertical([Constraint::Min(1), Constraint::Length(error_rows)]).areas(inner);
        frame.render_widget(&self.editor, text_area);
        let style = Style::default().fg(rgb_to_color(theme.status_deleted));
        let error: Vec<Line<'static>> = error.into_iter().map(Line::from).collect();
        frame.render_widget(Paragraph::new(error).style(style), error_area);
    }
}

//! The hover popup: what the language server says about a symbol, shown
//! next to it.
//!
//! The pointer resting on a symbol for [`DELAY`] asks the server; `i`
//! asks about the picked symbol at once. This module owns the wait, the
//! request in flight, the popup's text, scroll, and place, and when it
//! closes. Which symbol sits under the pointer, and whether it can be
//! asked about, is the caller's to say (see `navigate`).

mod markdown;

use std::ops::Range;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ratatui::layout::{Position, Rect};
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Clear, Paragraph};

use deltoids::render_tui::{pane_block_with_title_line, pane_border_color, rgb_to_color};
use deltoids::{Language, Theme};

use crate::cli::browse::comments::CommentAnchor;
use crate::cli::browse::text::display_width;
use crate::lsp::{Lsp, Query, Reply, Symbol, Ticket};

/// How long the pointer rests on a symbol before the popup asks.
const DELAY: Duration = Duration::from_millis(300);
/// The widest the popup's text gets, in columns.
const MAX_WIDTH: usize = 80;
/// The tallest the popup gets, borders included.
const MAX_HEIGHT: u16 = 20;

/// A symbol hover can ask about, and where it is on screen.
pub(super) struct Spot {
    /// The symbol's line in the version the pane shows.
    pub(super) anchor: CommentAnchor,
    /// Byte range of the symbol within its line.
    pub(super) columns: Range<usize>,
    /// The screen cells the symbol covers, one rect per wrapped row.
    pub(super) cells: Vec<Rect>,
    pub(super) name: String,
    pub(super) language: Language,
    /// The syntect syntax the file is highlighted as.
    pub(super) highlight: Option<String>,
    /// Workdir-relative.
    pub(super) path: String,
    /// The file on disk, which the server reads, and the symbol's line
    /// there (1-based).
    pub(super) disk: Arc<str>,
    pub(super) disk_line: usize,
}

impl Spot {
    fn same_symbol(&self, other: &Spot) -> bool {
        self.anchor == other.anchor && self.columns == other.columns
    }

    fn covers(&self, position: Position) -> bool {
        self.cells.iter().any(|cells| cells.contains(position))
    }
}

/// What a tick found.
#[derive(Default)]
pub(super) struct Tick {
    /// The popup appeared, closed, or changed.
    pub(super) changed: bool,
    /// A wait or a request is under way.
    pub(super) pending: bool,
    /// Why `i` showed nothing.
    pub(super) status: Option<String>,
}

enum State {
    Idle,
    /// The pointer is on `spot`, since `since`.
    Resting {
        spot: Spot,
        since: Instant,
    },
    /// Waiting for the server. `keyed` when `i` asked.
    Asking {
        spot: Spot,
        ticket: Ticket,
        keyed: bool,
    },
    Shown(Popup),
}

struct Popup {
    spot: Spot,
    text: String,
    scroll: usize,
    /// Where the last draw put the popup.
    area: Rect,
    /// The rendered lines, for the width and syntax theme they were
    /// rendered at.
    rendered: Option<(usize, String, Vec<Line<'static>>)>,
}

pub(super) struct Hover {
    lsp: Option<Lsp>,
    state: State,
    /// The screen changed since the last tick.
    changed: bool,
}

impl Hover {
    pub(super) fn new(lsp: Option<Lsp>) -> Self {
        Self {
            lsp,
            state: State::Idle,
            changed: false,
        }
    }

    /// Whether `position` is on the symbol hover is about, or on the
    /// popup: pointer moves there keep things as they are.
    pub(super) fn holds(&self, position: Position) -> bool {
        match &self.state {
            State::Idle => false,
            State::Resting { spot, .. } | State::Asking { spot, .. } => spot.covers(position),
            State::Shown(popup) => popup.spot.covers(position) || popup.area.contains(position),
        }
    }

    /// The pointer moved onto `spot`, or off any symbol hover can ask
    /// about: start waiting on a new symbol, or close.
    pub(super) fn rest(&mut self, spot: Option<Spot>) {
        let Some(spot) = spot else {
            self.close();
            return;
        };
        let same = match &self.state {
            State::Idle => false,
            State::Resting { spot: current, .. } | State::Asking { spot: current, .. } => {
                current.same_symbol(&spot)
            }
            State::Shown(popup) => popup.spot.same_symbol(&spot),
        };
        if same {
            return;
        }
        self.close();
        self.state = State::Resting {
            spot,
            since: Instant::now(),
        };
    }

    /// Ask about `spot` now (`i`). Fails when the server cannot be asked.
    pub(super) fn open(&mut self, spot: Spot) -> Result<(), String> {
        self.close();
        let ticket = self.ask(&spot)?;
        self.state = State::Asking {
            spot,
            ticket,
            keyed: true,
        };
        Ok(())
    }

    /// Close the popup and drop any wait or request. Reports whether
    /// there was a popup or a request from `i` to close.
    pub(super) fn close(&mut self) -> bool {
        let state = std::mem::replace(&mut self.state, State::Idle);
        match state {
            State::Idle | State::Resting { .. } => false,
            State::Asking { ticket, keyed, .. } => {
                if let Some(lsp) = &self.lsp {
                    lsp.cancel(ticket);
                }
                keyed
            }
            State::Shown(_) => {
                self.changed = true;
                true
            }
        }
    }

    /// Scroll the popup `step` lines when `position` is on it. Reports
    /// whether it was.
    pub(super) fn scroll(&mut self, position: Position, step: isize) -> bool {
        let State::Shown(popup) = &mut self.state else {
            return false;
        };
        if !popup.area.contains(position) {
            return false;
        }
        let before = popup.scroll;
        popup.scroll = popup.scroll.saturating_add_signed(step);
        self.changed |= popup.scroll != before;
        true
    }

    /// The symbol `i` is waiting on, for the footer.
    pub(super) fn asking(&self) -> Option<&str> {
        match &self.state {
            State::Asking {
                spot, keyed: true, ..
            } => Some(&spot.name),
            _ => None,
        }
    }

    /// Ask once the pointer has rested long enough, and collect the
    /// server's answer.
    pub(super) fn tick(&mut self) -> Tick {
        let mut tick = Tick::default();
        let state = std::mem::replace(&mut self.state, State::Idle);
        self.state = match state {
            State::Resting { spot, since } if since.elapsed() >= DELAY => match self.ask(&spot) {
                Ok(ticket) => State::Asking {
                    spot,
                    ticket,
                    keyed: false,
                },
                Err(_) => State::Idle,
            },
            State::Asking {
                spot,
                ticket,
                keyed,
            } => match self.lsp.as_ref().and_then(|lsp| lsp.answer(&ticket)) {
                None => State::Asking {
                    spot,
                    ticket,
                    keyed,
                },
                Some(Ok(Reply::Info(text))) => {
                    self.changed = true;
                    State::Shown(Popup {
                        spot,
                        text,
                        scroll: 0,
                        area: Rect::default(),
                        rendered: None,
                    })
                }
                Some(Ok(Reply::Places(_))) => State::Idle,
                Some(Err(message)) => {
                    if keyed {
                        tick.status = Some(message);
                    }
                    State::Idle
                }
            },
            state => state,
        };
        tick.pending = matches!(self.state, State::Resting { .. } | State::Asking { .. });
        tick.changed = std::mem::take(&mut self.changed);
        tick
    }

    fn ask(&self, spot: &Spot) -> Result<Ticket, String> {
        let lsp = self.lsp.as_ref().ok_or("No language server")?;
        lsp.ask(
            Query::Hover,
            Symbol {
                language: spot.language,
                path: &spot.path,
                text: &spot.disk,
                line: spot.disk_line,
                columns: spot.columns.clone(),
            },
        )
    }

    /// Draw the popup within `pane`, below its symbol, or above when
    /// there is more room there.
    pub(super) fn draw(&mut self, frame: &mut ratatui::Frame<'_>, pane: Rect, theme: &Theme) {
        let State::Shown(popup) = &mut self.state else {
            return;
        };
        let Some(&symbol) = popup.spot.cells.first() else {
            return;
        };
        let text_width = MAX_WIDTH.min(pane.width.saturating_sub(4) as usize);
        if text_width == 0 {
            return;
        }
        let fresh = popup.rendered.as_ref().is_some_and(|(width, syntax, _)| {
            *width == text_width && *syntax == theme.syntax_theme_name
        });
        if !fresh {
            let code = markdown::Code {
                language: popup.spot.language.name(),
                highlight: popup.spot.highlight.as_deref(),
            };
            let lines = markdown::render(&popup.text, &code, text_width, theme);
            popup.rendered = Some((text_width, theme.syntax_theme_name.clone(), lines));
        }
        let lines = &popup.rendered.as_ref().expect("rendered above").2;
        let used = lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| display_width(&span.content))
                    .sum::<usize>()
            })
            .max()
            .unwrap_or(0)
            .min(text_width);
        let width = used as u16 + 4;
        let wanted = (lines.len() as u16 + 2).min(MAX_HEIGHT);
        let below = pane.bottom().saturating_sub(symbol.y + 1);
        let above = symbol.y.saturating_sub(pane.y);
        let (height, y) = if wanted <= below || below >= above {
            let height = wanted.min(below);
            (height, symbol.y + 1)
        } else {
            let height = wanted.min(above);
            (height, symbol.y - height)
        };
        if height < 3 {
            popup.area = Rect::default();
            return;
        }
        let x = symbol.x.min(pane.right().saturating_sub(width)).max(pane.x);
        let area = Rect::new(x, y, width.min(pane.width), height);
        let inner_height = height as usize - 2;
        popup.scroll = popup.scroll.min(lines.len().saturating_sub(inner_height));
        popup.area = area;

        let muted = Style::default().fg(rgb_to_color(theme.muted));
        let more = popup.scroll + inner_height < lines.len();
        let footer = more.then(|| Line::styled(" ↓ more ", muted));
        let block =
            pane_block_with_title_line(Line::default(), pane_border_color(true, theme), footer);
        let visible: Vec<Line<'static>> = lines
            .iter()
            .skip(popup.scroll)
            .take(inner_height)
            .cloned()
            .collect();
        frame.render_widget(Clear, area);
        let inner = block.inner(area);
        frame.render_widget(block, area);
        let padded = Rect::new(
            inner.x + 1,
            inner.y,
            inner.width.saturating_sub(2),
            inner.height,
        );
        frame.render_widget(Paragraph::new(visible), padded);
    }
}

//! Draw any [`Mode`] onto an in-memory terminal and read the screen back
//! as text, waiting out its background rendering first.

use std::time::{Duration, Instant};

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use deltoids::Theme;
use deltoids::render_tui::pane_border_color;

use super::mode::{DrawBudget, Mode};
use super::{pane, split_columns};
use crate::sidebar::{IconMode, filter_icon};

pub(super) const WIDTH: u16 = 100;
pub(super) const HEIGHT: u16 = 24;
/// Width of the left column in [`screen`], its divider included.
pub(super) const LEFT: u16 = 40;
/// The right pane's first column in [`screen`].
pub(super) const RIGHT: u16 = LEFT;
/// Inner pane height for key handling, matching [`screen`].
pub(super) const PANE_HEIGHT: usize = HEIGHT as usize - 2;

/// The settled screen, one string per row.
pub(super) fn screen(mode: &mut impl Mode) -> Vec<String> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let rows = text(&draw(mode));
        let work = mode.background(true);
        if !work.pending && !work.changed {
            return rows;
        }
        assert!(Instant::now() < deadline, "background never settled");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// The left column of the settled screen.
pub(super) fn left_text(mode: &mut impl Mode) -> String {
    columns(mode).0
}

/// The right pane of the settled screen.
pub(super) fn right_text(mode: &mut impl Mode) -> String {
    columns(mode).1
}

fn columns(mode: &mut impl Mode) -> (String, String) {
    let rows = screen(mode);
    let split = |row: &String| {
        let chars: Vec<char> = row.chars().collect();
        let left: String = chars[..LEFT as usize - 1].iter().collect();
        let right: String = chars[RIGHT as usize..].iter().collect();
        (left, right)
    };
    let (left, right): (Vec<_>, Vec<_>) = rows.iter().map(split).unzip();
    (left.join("\n"), right.join("\n"))
}

/// Whether the right pane's title shows focus.
pub(super) fn right_focused(mode: &mut impl Mode) -> bool {
    screen(mode);
    title_focused(&draw(mode), RIGHT..WIDTH)
}

/// Whether the left column's title shows focus.
pub(super) fn left_focused(mode: &mut impl Mode) -> bool {
    screen(mode);
    title_focused(&draw(mode), 0..LEFT - 1)
}

/// Whether the title row draws text in the focus colour within
/// `columns`. The filter funnel is in the accent whatever the focus.
fn title_focused(buffer: &Buffer, columns: std::ops::Range<u16>) -> bool {
    let accent = pane_border_color(true, &Theme::default());
    let funnel = filter_icon(IconMode::from_env());
    columns.into_iter().any(|x| {
        let cell = &buffer[(x, 0)];
        cell.fg == accent && cell.symbol().trim() != "" && !funnel.starts_with(cell.symbol())
    })
}

fn text(buffer: &Buffer) -> Vec<String> {
    (0..HEIGHT)
        .map(|y| (0..WIDTH).map(|x| buffer[(x, y)].symbol()).collect())
        .collect()
}

/// One frame, without waiting for background work.
pub(super) fn draw(mode: &mut impl Mode) -> Buffer {
    let theme = Theme::default();
    let mut term = Terminal::new(TestBackend::new(WIDTH, HEIGHT)).unwrap();
    term.draw(|frame| {
        let area: Rect = frame.area();
        let (left, divider, right) = split_columns(area, LEFT);
        pane::draw_divider(frame, divider, area, &theme);
        mode.draw(frame, left, right, &theme, DrawBudget::Full);
        pane::draw_junction(frame, divider, area, &theme);
    })
    .unwrap();
    term.backend().buffer().clone()
}

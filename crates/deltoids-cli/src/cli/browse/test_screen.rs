//! Draw any [`Mode`] onto an in-memory terminal and read the screen back
//! as text, waiting out its background rendering first.

use std::time::{Duration, Instant};

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Direction, Layout};

use deltoids::Theme;
use deltoids::render_tui::pane_border_color;

use super::mode::{DrawBudget, Mode};

pub(super) const WIDTH: u16 = 100;
pub(super) const HEIGHT: u16 = 24;
/// Width of the left column in [`screen`].
pub(super) const LEFT: u16 = 40;
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
        let left: String = chars[..LEFT as usize].iter().collect();
        let right: String = chars[LEFT as usize..].iter().collect();
        (left, right)
    };
    let (left, right): (Vec<_>, Vec<_>) = rows.iter().map(split).unzip();
    (left.join("\n"), right.join("\n"))
}

/// Whether the right pane's border shows focus.
pub(super) fn right_focused(mode: &mut impl Mode) -> bool {
    screen(mode);
    let corner = &draw(mode)[(LEFT, 0)];
    corner.fg == pane_border_color(true, &Theme::default())
}

/// Whether the left column's border cell on `row` shows focus.
pub(super) fn left_border_focused(mode: &mut impl Mode, row: usize) -> bool {
    screen(mode);
    let cell = &draw(mode)[(0, row as u16)];
    cell.fg == pane_border_color(true, &Theme::default())
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
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(LEFT), Constraint::Min(10)])
            .split(frame.area());
        mode.draw(frame, cols[0], cols[1], &theme, DrawBudget::Full);
    })
    .unwrap();
    term.backend().buffer().clone()
}

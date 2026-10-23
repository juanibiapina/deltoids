//! Shared help popup: the sectioned binding tables that are the source
//! of truth for the popup, the popup's own key and wheel handling
//! (close and scroll), and its centered overlay render. The shell holds
//! an open [`HelpPopup`] and drives this slice.

use crossterm::event::{KeyCode, MouseEventKind};
use ratatui::layout::{Margin, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use unicode_width::UnicodeWidthStr;

use deltoids::Theme;
use deltoids::render_tui::{pane_block, pane_border_color, rgb_to_color};

use super::command::CustomCommand;

/// A titled group of `(binding, description)` rows in the help popup.
pub(super) struct HelpSection {
    pub(super) title: &'static str,
    pub(super) rows: &'static [(&'static str, &'static str)],
}

/// Key bindings, grouped by where they apply. Drawn before the custom
/// commands and the mouse section.
pub(super) const KEY_SECTIONS: &[HelpSection] = &[
    HelpSection {
        title: "General",
        rows: &[
            ("?", "toggle this help"),
            ("t", "pick syntax theme"),
            ("\\", "cycle diff layout (grouped / interleaved)"),
            ("< / >", "narrow / widen sidebar"),
            ("q", "quit"),
        ],
    },
    HelpSection {
        title: "Panels",
        rows: &[
            ("Tab / Shift+Tab", "next / previous side panel"),
            ("h / l", "previous / next side panel"),
            ("Enter", "open a file's diff, a branch, or a commit"),
            ("Esc", "back out / close review"),
        ],
    },
    HelpSection {
        title: "Moving",
        rows: &[
            ("j / k", "move in a list or between diff lines"),
            ("Shift+J / K", "scroll diff three lines"),
            ("PgDn / PgUp", "page in current pane"),
            ("g / G, Home / End", "top / bottom of current pane"),
        ],
    },
    HelpSection {
        title: "Changes",
        rows: &[
            ("s", "switch staged / unstaged diff"),
            ("f", "hide / show low-value files"),
            ("F", "hide / show low-value hunks"),
        ],
    },
    HelpSection {
        title: "Files sidebar",
        rows: &[
            ("Enter", "fold / unfold directory"),
            ("Space", "stage / unstage selection"),
            ("a", "stage / unstage all files"),
            ("d", "discard selection"),
            ("z / x", "expand / shrink hunks of selection"),
        ],
    },
    HelpSection {
        title: "Diff",
        rows: &[
            ("Tab", "switch staged / unstaged diff"),
            ("z / x", "expand / shrink hunk under the cursor"),
            ("w / b", "pick the next / previous symbol on the line"),
            ("Enter", "go to the picked symbol's definition"),
            ("r", "list where the picked symbol is used"),
            ("Ctrl+o", "go back to before the last jump"),
            ("Esc", "cancel a lookup / close the callers list"),
        ],
    },
    HelpSection {
        title: "Comments",
        rows: &[
            ("c", "comment on the line under the cursor"),
            ("d", "delete the comment under the cursor"),
            ("y", "copy comments"),
            ("D", "clear all comments"),
        ],
    },
];

/// Mouse gestures. Drawn last, after any custom commands.
pub(super) const MOUSE_SECTION: HelpSection = HelpSection {
    title: "Mouse",
    rows: &[
        ("Click", "focus the panel under the pointer"),
        ("Click a tab", "show its side panel"),
        ("Click a file", "select it"),
        ("Click ▶ / ▼", "fold / unfold a directory"),
        ("Click a diff line", "move the cursor there"),
        ("Click a file or hunk in a summary", "open the file there"),
        ("Click a symbol", "pick it"),
        ("Double-click a symbol", "go to its definition"),
        ("Right-click a symbol", "list where it is used"),
        ("Double-click", "open a branch or commit, like Enter"),
        ("Wheel", "scroll the pane under the pointer"),
        ("Shift / Ctrl+wheel", "step through files"),
        ("Drag the divider", "resize the sidebar"),
    ],
};

const CUSTOM_TITLE: &str = "Custom commands";

/// Blank columns between two popup columns.
const COLUMN_GAP: usize = 3;
/// Blank columns between a binding and its description.
const KEY_GAP: usize = 2;

/// What the popup does with a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum HelpKey {
    /// The popup stays open (it may have scrolled).
    Stay,
    /// `?` or `Esc`: the shell drops the popup.
    Close,
}

/// The open help popup. Holds the scroll offset, clamped against the
/// last drawn size, so a body taller than the screen stays reachable.
#[derive(Debug, Default)]
pub(super) struct HelpPopup {
    scroll: usize,
    max_scroll: usize,
    page: usize,
}

impl HelpPopup {
    /// `?` and `Esc` close the popup; `j`/`k`, arrows, `PgDn`/`PgUp`,
    /// `g`/`G`, and `Home`/`End` scroll it; everything else is
    /// swallowed.
    pub(super) fn handle_key(&mut self, key: KeyCode) -> HelpKey {
        let page = self.page.max(1);
        self.scroll = match key {
            KeyCode::Char('?') | KeyCode::Esc => return HelpKey::Close,
            KeyCode::Char('j') | KeyCode::Down => self.scroll + 1,
            KeyCode::Char('k') | KeyCode::Up => self.scroll.saturating_sub(1),
            KeyCode::PageDown => self.scroll + page,
            KeyCode::PageUp => self.scroll.saturating_sub(page),
            KeyCode::Char('g') | KeyCode::Home => 0,
            KeyCode::Char('G') | KeyCode::End => usize::MAX,
            _ => self.scroll,
        }
        .min(self.max_scroll);
        HelpKey::Stay
    }

    /// The wheel scrolls the popup.
    pub(super) fn handle_mouse(&mut self, kind: MouseEventKind) {
        match kind {
            MouseEventKind::ScrollDown => self.handle_key(KeyCode::Down),
            MouseEventKind::ScrollUp => self.handle_key(KeyCode::Up),
            _ => HelpKey::Stay,
        };
    }

    /// Render the popup as a centered, bordered overlay. Sized to
    /// content (capped at 80% of the terminal in each axis), cleared
    /// underneath so the panes don't bleed through. Sections flow into
    /// as many side-by-side columns as fit the width; when the body is
    /// still taller than the popup, it scrolls and the title says so.
    pub(super) fn draw(
        &mut self,
        frame: &mut ratatui::Frame<'_>,
        area: Rect,
        theme: &Theme,
        commands: &[CustomCommand],
    ) {
        let sections = help_sections(commands);
        let max_w = (area.width * 8 / 10).max(20).min(area.width);
        let max_h = (area.height * 8 / 10).max(5).min(area.height);
        let inner_w = usize::from(max_w.saturating_sub(4)); // 2 borders + 2 padding
        let inner_h = usize::from(max_h.saturating_sub(2)); // 2 borders
        let columns = choose_columns(&sections, inner_w, inner_h);
        let (rows, content_width) = build_help_lines(&sections, &columns, theme);

        let w = (content_width as u16).saturating_add(4).min(max_w);
        let h = (rows.len() as u16).saturating_add(2).min(max_h);
        let popup = Rect {
            x: area.x + (area.width.saturating_sub(w)) / 2,
            y: area.y + (area.height.saturating_sub(h)) / 2,
            width: w,
            height: h,
        };

        self.page = usize::from(h.saturating_sub(2));
        self.max_scroll = rows.len().saturating_sub(self.page);
        self.scroll = self.scroll.min(self.max_scroll);
        let title = if self.max_scroll > 0 {
            "─Help · j/k to scroll─"
        } else {
            "─Help─"
        };

        frame.render_widget(Clear, popup);
        let block = pane_block(title, pane_border_color(true, theme));
        let inner = block.inner(popup).inner(Margin {
            vertical: 0,
            horizontal: 1,
        });
        frame.render_widget(block, popup);
        let scroll = u16::try_from(self.scroll).unwrap_or(u16::MAX);
        frame.render_widget(Paragraph::new(rows).scroll((scroll, 0)), inner);
    }
}

/// A help section ready to draw: built-in tables plus the runtime
/// custom-command rows.
struct Section {
    title: String,
    rows: Vec<(String, String)>,
}

impl Section {
    fn from_static(section: &HelpSection) -> Self {
        Self {
            title: section.title.to_string(),
            rows: section
                .rows
                .iter()
                .map(|(k, d)| ((*k).to_string(), (*d).to_string()))
                .collect(),
        }
    }

    /// Lines the section takes: its title plus one per row.
    fn height(&self) -> usize {
        self.rows.len() + 1
    }

    fn key_width(&self) -> usize {
        self.rows.iter().map(|(k, _)| k.width()).max().unwrap_or(0)
    }

    fn width(&self, key_w: usize) -> usize {
        self.rows
            .iter()
            .map(|(_, d)| key_w + KEY_GAP + d.width())
            .chain([self.title.width()])
            .max()
            .unwrap_or(0)
    }
}

/// Every section in drawing order: the key sections, a custom-command
/// section when any are configured, then the mouse section. Subprocess
/// commands get a trailing marker so the two run modes are
/// distinguishable.
fn help_sections(commands: &[CustomCommand]) -> Vec<Section> {
    let mut sections: Vec<Section> = KEY_SECTIONS.iter().map(Section::from_static).collect();
    if !commands.is_empty() {
        let rows = commands
            .iter()
            .map(|cmd| {
                let desc = if cmd.subprocess {
                    format!("{} (subprocess)", cmd.description)
                } else {
                    cmd.description.clone()
                };
                (cmd.key.to_string(), desc)
            })
            .collect();
        sections.push(Section {
            title: CUSTOM_TITLE.to_string(),
            rows,
        });
    }
    sections.push(Section::from_static(&MOUSE_SECTION));
    sections
}

/// One popup column: a run of consecutive section indices.
type Column = std::ops::Range<usize>;

/// Height of a column: its sections plus one blank line between each.
fn column_height(sections: &[Section], column: &Column) -> usize {
    let lines: usize = sections[column.clone()].iter().map(Section::height).sum();
    lines + column.len().saturating_sub(1)
}

/// Width of a column: the key column is aligned across its sections.
fn column_width(sections: &[Section], column: &Column) -> usize {
    let group = &sections[column.clone()];
    let key_w = group.iter().map(Section::key_width).max().unwrap_or(0);
    group.iter().map(|s| s.width(key_w)).max().unwrap_or(0)
}

/// Fill columns in order, starting a new one when the next section
/// would push the current column past `limit` lines.
fn pack(sections: &[Section], limit: usize) -> Vec<Column> {
    let mut columns: Vec<Column> = Vec::new();
    for i in 0..sections.len() {
        match columns.last_mut() {
            Some(column) if column_height(sections, &(column.start..i + 1)) <= limit => {
                column.end = i + 1;
            }
            _ => columns.push(i..i + 1),
        }
    }
    columns
}

/// The most even split into at most `n` columns: the smallest line
/// limit whose packing needs no more than `n` columns.
fn balanced(sections: &[Section], n: usize) -> Vec<Column> {
    let all = 0..sections.len();
    let tallest = sections.iter().map(Section::height).max().unwrap_or(0);
    (tallest..=column_height(sections, &all))
        .map(|limit| pack(sections, limit))
        .find(|columns| columns.len() <= n)
        .unwrap_or_else(|| vec![all])
}

/// The fewest columns whose height fits `max_h`, among those whose
/// width fits `max_w`. One column when nothing wider fits.
fn choose_columns(sections: &[Section], max_w: usize, max_h: usize) -> Vec<Column> {
    let mut best = balanced(sections, 1);
    for n in 1..=sections.len() {
        let columns = balanced(sections, n);
        let width = columns
            .iter()
            .map(|c| column_width(sections, c))
            .sum::<usize>()
            + COLUMN_GAP * columns.len().saturating_sub(1);
        if n > 1 && width > max_w {
            break;
        }
        let height = columns
            .iter()
            .map(|c| column_height(sections, c))
            .max()
            .unwrap_or(0);
        best = columns;
        if height <= max_h {
            break;
        }
    }
    best
}

/// Build the popup body as ratatui [`Line`]s, with `columns` laid side
/// by side. Returns the lines and the content width.
fn build_help_lines(
    sections: &[Section],
    columns: &[Column],
    theme: &Theme,
) -> (Vec<Line<'static>>, usize) {
    let title_style = Style::default().add_modifier(Modifier::BOLD);
    let key_style = Style::default().fg(rgb_to_color(theme.border_active));
    let desc_style = Style::default().fg(rgb_to_color(theme.muted));

    let rendered: Vec<(Vec<Vec<Span<'static>>>, usize)> = columns
        .iter()
        .map(|column| {
            let group = &sections[column.clone()];
            let key_w = group.iter().map(Section::key_width).max().unwrap_or(0);
            let mut lines: Vec<Vec<Span<'static>>> = Vec::new();
            for (i, section) in group.iter().enumerate() {
                if i > 0 {
                    lines.push(Vec::new());
                }
                lines.push(vec![Span::styled(section.title.clone(), title_style)]);
                for (k, d) in &section.rows {
                    lines.push(vec![
                        Span::styled(k.clone(), key_style),
                        Span::raw(" ".repeat(key_w - k.width() + KEY_GAP)),
                        Span::styled(d.clone(), desc_style),
                    ]);
                }
            }
            (lines, column_width(sections, column))
        })
        .collect();

    let height = rendered.iter().map(|(l, _)| l.len()).max().unwrap_or(0);
    let lines = (0..height)
        .map(|row| {
            let mut spans: Vec<Span<'static>> = Vec::new();
            let mut used = 0;
            let mut offset = 0;
            for (lines, width) in &rendered {
                if let Some(line) = lines.get(row).filter(|l| !l.is_empty()) {
                    spans.push(Span::raw(" ".repeat(offset - used)));
                    used = offset + line.iter().map(|s| s.content.width()).sum::<usize>();
                    spans.extend(line.iter().cloned());
                }
                offset += width + COLUMN_GAP;
            }
            Line::from(spans)
        })
        .collect();
    let width = rendered.iter().map(|(_, w)| w).sum::<usize>()
        + COLUMN_GAP * rendered.len().saturating_sub(1);
    (lines, width)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn all_key_rows() -> impl Iterator<Item = &'static (&'static str, &'static str)> {
        KEY_SECTIONS.iter().flat_map(|s| s.rows.iter())
    }

    fn is_mouse_gesture(binding: &str) -> bool {
        let binding = binding.to_lowercase();
        ["click", "wheel", "drag"]
            .iter()
            .any(|word| binding.contains(word))
    }

    fn commands() -> Vec<CustomCommand> {
        vec![
            CustomCommand {
                key: 'e',
                command: "dev tmux edit {{filename}}".into(),
                subprocess: false,
                description: "edit in a tmux pane".into(),
            },
            CustomCommand {
                key: 'E',
                command: "nvim {{filename}}".into(),
                subprocess: true,
                description: "edit inline".into(),
            },
        ]
    }

    /// Draw `popup` on a `width` x `height` terminal; one string per row.
    fn draw(
        popup: &mut HelpPopup,
        width: u16,
        height: u16,
        commands: &[CustomCommand],
    ) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        let theme = Theme::default();
        terminal
            .draw(|frame| popup.draw(frame, frame.area(), &theme, commands))
            .unwrap();
        let buffer = terminal.backend().buffer();
        (0..height)
            .map(|y| (0..width).map(|x| buffer[(x, y)].symbol()).collect())
            .collect()
    }

    fn screen(width: u16, height: u16, commands: &[CustomCommand]) -> Vec<String> {
        draw(&mut HelpPopup::default(), width, height, commands)
    }

    /// Screen column where `title` starts, if it is drawn as a title row.
    fn title_column(screen: &[String], title: &str) -> Option<usize> {
        screen.iter().find_map(|row| {
            let at = row.find(title)?;
            Some(row[..at].chars().count())
        })
    }

    #[test]
    fn help_keys_document_clear_all_comments() {
        let clear = all_key_rows()
            .find(|(k, _)| *k == "D")
            .expect("help popup must document the clear-all binding");
        assert!(clear.1.contains("clear"));
    }

    #[test]
    fn help_keys_document_q_as_the_only_quit_key() {
        let quit = all_key_rows()
            .find(|(_, d)| d.contains("quit"))
            .expect("help popup must document the quit binding");
        assert_eq!(quit.0, "q");
        assert!(
            !all_key_rows().any(|(k, d)| k.contains("Esc") && d.contains("quit")),
            "Esc must not be documented as a quit key"
        );
    }

    #[test]
    fn mouse_gestures_live_only_in_the_mouse_section() {
        for (binding, _) in all_key_rows() {
            assert!(
                !is_mouse_gesture(binding),
                "{binding:?} is a mouse gesture listed among the keys"
            );
        }
        for (binding, _) in MOUSE_SECTION.rows {
            assert!(
                is_mouse_gesture(binding),
                "{binding:?} is a key listed in the mouse section"
            );
        }
    }

    #[test]
    fn custom_commands_get_a_section_before_mouse() {
        let sections = help_sections(&commands());
        let titles: Vec<&str> = sections.iter().map(|s| s.title.as_str()).collect();
        assert_eq!(titles[titles.len() - 2..], [CUSTOM_TITLE, "Mouse"]);
        let custom = &sections[sections.len() - 2];
        assert_eq!(
            custom.rows,
            [
                ("e".to_string(), "edit in a tmux pane".to_string()),
                ("E".to_string(), "edit inline (subprocess)".to_string()),
            ]
        );
    }

    #[test]
    fn no_custom_commands_means_no_custom_section() {
        let sections = help_sections(&[]);
        assert!(sections.iter().all(|s| s.title != CUSTOM_TITLE));
        assert_eq!(sections.last().unwrap().title, "Mouse");
    }

    #[test]
    fn a_tall_terminal_shows_one_column() {
        let screen = screen(100, 80, &commands());
        let general = title_column(&screen, "General").expect("General drawn");
        let mouse = title_column(&screen, "Mouse").expect("Mouse drawn");
        assert_eq!(general, mouse);
    }

    #[test]
    fn a_wide_short_terminal_flows_sections_into_columns() {
        let screen = screen(220, 40, &commands());
        for section in help_sections(&commands()) {
            assert!(
                title_column(&screen, &section.title).is_some(),
                "{} is cut off",
                section.title
            );
        }
        let general = title_column(&screen, "General").unwrap();
        let mouse = title_column(&screen, "Mouse").unwrap();
        assert!(mouse > general, "Mouse should sit in a later column");
    }

    #[test]
    fn a_short_narrow_terminal_scrolls_to_the_mouse_section() {
        let mut popup = HelpPopup::default();
        let first = draw(&mut popup, 100, 30, &[]);
        assert!(title_column(&first, "General").is_some());
        assert!(title_column(&first, "Mouse").is_none());
        assert!(
            first[3].contains("j/k to scroll"),
            "title hints at scrolling"
        );

        assert_eq!(popup.handle_key(KeyCode::Char('G')), HelpKey::Stay);
        let last = draw(&mut popup, 100, 30, &[]);
        assert!(title_column(&last, "Mouse").is_some());
        assert!(last.iter().any(|row| row.contains("Drag the divider")));

        popup.handle_key(KeyCode::Char('g'));
        let top = draw(&mut popup, 100, 30, &[]);
        assert_eq!(top, first);
    }

    #[test]
    fn scrolling_stops_at_the_last_row() {
        let mut popup = HelpPopup::default();
        draw(&mut popup, 100, 30, &[]);
        popup.handle_key(KeyCode::Char('G'));
        let bottom = draw(&mut popup, 100, 30, &[]);
        popup.handle_key(KeyCode::Char('j'));
        assert_eq!(draw(&mut popup, 100, 30, &[]), bottom);
        popup.handle_key(KeyCode::Char('k'));
        assert_ne!(
            draw(&mut popup, 100, 30, &[]),
            bottom,
            "one k moves back up"
        );
    }

    #[test]
    fn question_mark_and_esc_close_the_popup() {
        let mut popup = HelpPopup::default();
        assert_eq!(popup.handle_key(KeyCode::Char('?')), HelpKey::Close);
        assert_eq!(popup.handle_key(KeyCode::Esc), HelpKey::Close);
    }

    #[test]
    fn other_keys_keep_the_popup_open() {
        let mut popup = HelpPopup::default();
        assert_eq!(popup.handle_key(KeyCode::Char('x')), HelpKey::Stay);
        assert_eq!(popup.handle_key(KeyCode::Char('j')), HelpKey::Stay);
    }
}

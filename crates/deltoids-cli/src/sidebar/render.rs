//! Row-rendering axis: turn one [`Row`] plus its [`FileRowMeta`] into a
//! styled [`Line`], including status badges, icons, delta counts, and the
//! trailing binary/mode/submodule badges.

use deltoids::Theme;
use deltoids::parse::FileDiff;
use deltoids::render_tui::rgb_to_color;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use super::FileRowMeta;
use super::icons::{ICON_DIR_OPEN, IconMode, file_icon};
use super::status::{FileMode, FileStatus, ModeChange};
use super::tree::Row;

/// Just the basename (last `/`-separated segment) of a path.
pub(super) fn rename_leaf(path: &str) -> String {
    path.rsplit('/').next().unwrap_or(path).to_string()
}

/// Pull the source path from a `copy from` line in the preamble.
pub(super) fn copy_origin(file: &FileDiff) -> Option<String> {
    file.preamble.iter().find_map(|line| {
        line.trim_start()
            .strip_prefix("copy from ")
            .map(str::to_owned)
    })
}

/// Short label describing a [`ModeChange`].
///
/// Executable bit toggles render as `+x` / `-x`. Real type changes
/// render as `regular→symlink`, etc., so the user sees what the file
/// turned into.
fn mode_change_label(change: ModeChange) -> String {
    match change {
        ModeChange::ExecutableSet => "+x".to_string(),
        ModeChange::ExecutableCleared => "-x".to_string(),
        ModeChange::TypeChange { old, new } => {
            format!("{}→{}", file_mode_label(old), file_mode_label(new))
        }
    }
}

fn file_mode_label(mode: FileMode) -> &'static str {
    match mode {
        FileMode::Regular => "file",
        FileMode::Executable => "exec",
        FileMode::Symlink => "symlink",
        FileMode::Submodule => "submodule",
        FileMode::Other => "?",
    }
}

pub(super) fn render_row(
    row: &Row,
    meta: &FileRowMeta,
    selected: bool,
    icons: IconMode,
    theme: &Theme,
) -> Line<'static> {
    let mut spans = Vec::new();

    let bg = if selected {
        Some(rgb_to_color(theme.selection_bg))
    } else {
        None
    };
    let base = match bg {
        Some(c) => Style::default().bg(c),
        None => Style::default(),
    };

    match row {
        Row::Dir { label, depth } => {
            spans.push(Span::styled(indent(*depth), base));
            if icons == IconMode::On {
                spans.push(Span::styled(
                    format!("{} ", ICON_DIR_OPEN),
                    base.fg(rgb_to_color(theme.border)),
                ));
            }
            spans.push(Span::styled(
                label.clone(),
                base.fg(rgb_to_color(theme.muted))
                    .add_modifier(Modifier::BOLD),
            ));
        }
        Row::File { name, depth, .. } => {
            spans.push(Span::styled(indent(*depth), base));

            if let Some(status) = meta.status {
                let badge = format!("{} ", status.badge());
                spans.push(Span::styled(
                    badge,
                    base.fg(status_color(status)).add_modifier(Modifier::BOLD),
                ));
            }

            if icons == IconMode::On {
                let icon = file_icon(name);
                spans.push(Span::styled(
                    format!("{} ", icon),
                    base.fg(rgb_to_color(theme.border)),
                ));
            }

            let display_name = match &meta.rename {
                Some((old, new)) => format!("{old} \u{2192} {new}"),
                None => name.clone(),
            };
            spans.push(Span::styled(display_name, base));

            if let Some((added, deleted)) = meta.deltas {
                if added > 0 || deleted > 0 {
                    spans.push(Span::styled(" ".to_string(), base));
                }
                if added > 0 {
                    spans.push(Span::styled(format!("+{added}"), base.fg(Color::Green)));
                }
                if added > 0 && deleted > 0 {
                    spans.push(Span::styled(" ".to_string(), base));
                }
                if deleted > 0 {
                    spans.push(Span::styled(format!("-{deleted}"), base.fg(Color::Red)));
                }
            }

            // Trailing badges: binary, mode change, submodule. These
            // sit at the right of the row in the muted/border colour
            // so they don't compete with the status badge.
            let muted = base.fg(rgb_to_color(theme.muted));
            if meta.extra.binary {
                spans.push(Span::styled(" (binary)".to_string(), muted));
            }
            if let Some(change) = meta.extra.mode_change {
                spans.push(Span::styled(
                    format!(" ({})", mode_change_label(change)),
                    muted,
                ));
            }
            if meta.extra.is_submodule {
                spans.push(Span::styled(" (submodule)".to_string(), muted));
            }
        }
    }

    Line::from(spans)
}

fn indent(depth: usize) -> String {
    "  ".repeat(depth)
}

fn status_color(status: FileStatus) -> Color {
    match status {
        FileStatus::Added => Color::Green,
        FileStatus::Deleted => Color::Red,
        FileStatus::Modified => Color::Yellow,
        FileStatus::Renamed => Color::Cyan,
        FileStatus::Copied => Color::Cyan,
        FileStatus::TypeChanged => Color::Magenta,
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Sidebar, SidebarFile};
    use super::*;
    use crate::sidebar::test_support::*;

    #[test]
    fn rendered_binary_file_row_has_binary_badge() {
        let f = fd_with_preamble(
            "image.png",
            &["Binary files a/image.png and b/image.png differ"],
        );
        let files = vec![SidebarFile {
            file: &f,
            added: 0,
            deleted: 0,
        }];
        let sidebar = Sidebar::build_with_icons(&files, &theme(), IconMode::Off);
        let combined = sidebar
            .rows()
            .iter()
            .map(line_text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            combined.contains("(binary)"),
            "missing binary badge: {combined}"
        );
    }

    #[test]
    fn rendered_mode_change_row_has_executable_badge() {
        let f = fd_with_preamble("script.sh", &["old mode 100644", "new mode 100755"]);
        let files = vec![SidebarFile {
            file: &f,
            added: 0,
            deleted: 0,
        }];
        let sidebar = Sidebar::build_with_icons(&files, &theme(), IconMode::Off);
        let combined = sidebar
            .rows()
            .iter()
            .map(line_text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(combined.contains("(+x)"), "missing +x badge: {combined}");
    }

    #[test]
    fn rendered_copied_file_shows_old_arrow_new_and_c_status() {
        let mut f = fd("new.rs");
        f.preamble = vec![
            "diff --git a/old.rs b/new.rs".to_string(),
            "similarity index 100%".to_string(),
            "copy from old.rs".to_string(),
            "copy to new.rs".to_string(),
        ];
        f.old_hash = Some("a".repeat(40));
        f.new_hash = Some("b".repeat(40));
        let files = vec![SidebarFile {
            file: &f,
            added: 0,
            deleted: 0,
        }];
        let sidebar = Sidebar::build_with_icons(&files, &theme(), IconMode::Off);
        let combined = sidebar
            .rows()
            .iter()
            .map(line_text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(combined.contains('C'), "missing C status: {combined}");
        assert!(
            combined.contains("old.rs \u{2192} new.rs"),
            "missing copy arrow: {combined}"
        );
    }

    #[test]
    fn rendered_file_row_contains_status_and_name() {
        let f = fd_added("hello.rs");
        let files = vec![SidebarFile {
            file: &f,
            added: 12,
            deleted: 0,
        }];
        let sidebar = Sidebar::build_with_icons(&files, &theme(), IconMode::Off);
        let rows = sidebar.rows();
        // Find the file row.
        let file_row = rows
            .iter()
            .find(|r| line_text(r).contains("hello.rs"))
            .expect("file row not found");
        let text = line_text(file_row);
        assert!(text.contains('A'), "missing status A in {text:?}");
        assert!(text.contains("+12"), "missing +12 in {text:?}");
    }

    #[test]
    fn rendered_renamed_file_shows_old_arrow_new() {
        let f = fd_renamed("src/old.rs", "src/new.rs");
        let files = vec![SidebarFile {
            file: &f,
            added: 1,
            deleted: 1,
        }];
        let sidebar = Sidebar::build_with_icons(&files, &theme(), IconMode::Off);
        let combined = sidebar
            .rows()
            .iter()
            .map(line_text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            combined.contains("old.rs \u{2192} new.rs"),
            "missing rename arrow in: {combined}"
        );
    }

    #[test]
    fn rendered_directory_row_contains_label() {
        let f = fd("src/a.rs");
        let files = vec![SidebarFile {
            file: &f,
            added: 0,
            deleted: 0,
        }];
        let sidebar = Sidebar::build_with_icons(&files, &theme(), IconMode::Off);
        let rows = sidebar.rows();
        let dir = rows.first().expect("at least one row");
        assert!(line_text(dir).contains("src/"));
    }

    #[test]
    fn icons_off_omits_icon_glyphs() {
        let f = fd("a.rs");
        let files = vec![SidebarFile {
            file: &f,
            added: 0,
            deleted: 0,
        }];
        let sidebar = Sidebar::build_with_icons(&files, &theme(), IconMode::Off);
        let row = &sidebar.rows()[0];
        let text = line_text(row);
        assert!(
            !text.contains('\u{e7a8}') && !text.contains('\u{f15b}'),
            "expected no file icon, got {text:?}"
        );
    }

    #[test]
    fn icons_on_includes_icon_glyph_for_known_extension() {
        let f = fd("a.rs");
        let files = vec![SidebarFile {
            file: &f,
            added: 0,
            deleted: 0,
        }];
        let sidebar = Sidebar::build_with_icons(&files, &theme(), IconMode::On);
        let combined = sidebar
            .rows()
            .iter()
            .map(line_text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(combined.contains('\u{e7a8}'), "missing rust icon");
    }
}

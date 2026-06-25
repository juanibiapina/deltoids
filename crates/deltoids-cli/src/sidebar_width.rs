//! Single source of truth for sidebar sizing, shared by both TUIs
//! (`review` and `traces`).
//!
//! The whole sizing policy — the terminal fraction, the min/max clamp,
//! the resize step, and the divider-drag math — lives behind a handful
//! of verbs. Callers pass only the terminal width and the user's
//! intent; the constants never leak. Neither sidebar ever hides:
//! [`Preference::effective`] always returns a width of at least
//! [`MIN_SIDEBAR_WIDTH`], clamping on narrow terminals instead of
//! disappearing.

/// Numerator/denominator of the terminal width used as the default
/// sidebar outer width (borders included). 1/5 keeps the tree compact
/// while still scaling with the terminal: a 120-col terminal floors at
/// [`MIN_SIDEBAR_WIDTH`] (24), a 200-col terminal gets ~40.
const SIDEBAR_WIDTH_NUM: u16 = 1;
const SIDEBAR_WIDTH_DEN: u16 = 5;
/// Smallest the sidebar may be shrunk to (outer width, includes borders).
const MIN_SIDEBAR_WIDTH: u16 = 24;
/// Smallest the diff/content pane may be squeezed to when the sidebar
/// grows. Bounds how wide the sidebar can get.
const MIN_DIFF_WIDTH: u16 = 24;
/// Columns added/removed per `<`/`>` keypress.
const SIDEBAR_RESIZE_STEP: u16 = 4;

/// Default sidebar outer width (borders included) for a terminal of
/// `terminal_width`: a fixed fraction of the terminal, clamped to
/// `[MIN_SIDEBAR_WIDTH, terminal_width - MIN_DIFF_WIDTH]` so it never
/// starves the content pane. Used to seed a [`Preference`].
pub fn default_width(terminal_width: u16) -> u16 {
    clamp_width(
        terminal_width.saturating_mul(SIDEBAR_WIDTH_NUM) / SIDEBAR_WIDTH_DEN,
        terminal_width,
    )
}

/// Content/diff pane width left after a sidebar of `sidebar_width` and
/// the content pane's own two borders. Saturates to 0 on a tiny
/// terminal (never panics).
pub fn diff_pane_width(sidebar_width: u16, terminal_width: u16) -> usize {
    terminal_width.saturating_sub(sidebar_width + 2) as usize
}

/// Clamp a candidate sidebar width to `[MIN_SIDEBAR_WIDTH, terminal -
/// MIN_DIFF_WIDTH]`. The upper bound is forced to be at least
/// `MIN_SIDEBAR_WIDTH` so the clamp never panics on a tiny terminal and
/// always returns a width `>= MIN_SIDEBAR_WIDTH` — the sidebar wins the
/// space and the content pane degrades gracefully instead of hiding.
fn clamp_width(candidate: u16, terminal_width: u16) -> u16 {
    let max = terminal_width
        .saturating_sub(MIN_DIFF_WIDTH)
        .max(MIN_SIDEBAR_WIDTH);
    candidate.clamp(MIN_SIDEBAR_WIDTH, max)
}

/// A user's preferred sidebar width plus the policy to resolve it to an
/// on-screen width each frame. Both TUIs own one. Stores the raw
/// preference; clamping happens in [`Preference::effective`]. The
/// resize verbs hide the step size and floor.
#[derive(Debug, Clone, Copy)]
pub struct Preference {
    preferred: u16,
}

impl Preference {
    /// Seed from terminal width via [`default_width`].
    pub fn seeded(terminal_width: u16) -> Self {
        Self {
            preferred: default_width(terminal_width),
        }
    }

    /// On-screen sidebar width this frame: the preference clamped to
    /// `[MIN_SIDEBAR_WIDTH, terminal - MIN_DIFF_WIDTH]`. Always
    /// `>= MIN_SIDEBAR_WIDTH`; never hides.
    pub fn effective(self, terminal_width: u16) -> u16 {
        clamp_width(self.preferred, terminal_width)
    }

    /// One `>` step wider.
    pub fn widen(&mut self) {
        self.preferred = self.preferred.saturating_add(SIDEBAR_RESIZE_STEP);
    }

    /// One `<` step narrower, floored at the minimum.
    pub fn narrow(&mut self) {
        self.preferred = self
            .preferred
            .saturating_sub(SIDEBAR_RESIZE_STEP)
            .max(MIN_SIDEBAR_WIDTH);
    }

    /// Absolute set from a divider drag at terminal column `column`. The
    /// sidebar's right border sits at `column`, so its outer width is
    /// `column + 1`, floored at the minimum.
    pub fn set_from_divider(&mut self, column: u16) {
        self.preferred = column.saturating_add(1).max(MIN_SIDEBAR_WIDTH);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_width_scales_with_terminal() {
        // Wider terminal yields a wider default tree.
        assert!(default_width(200) > default_width(80));
    }

    #[test]
    fn default_width_leaves_diff_room() {
        // Even on a very wide terminal the default keeps MIN_DIFF_WIDTH
        // for the content pane.
        let w = 2000;
        assert!(default_width(w) <= w - MIN_DIFF_WIDTH);
    }

    #[test]
    fn default_width_clamps_to_min_on_narrow() {
        // Tiny/degenerate widths never go below the floor and never panic.
        assert_eq!(default_width(0), MIN_SIDEBAR_WIDTH);
        assert_eq!(default_width(10), MIN_SIDEBAR_WIDTH);
    }

    #[test]
    fn effective_never_hides_on_narrow_terminal() {
        // The old hide-on-narrow rule is gone: a narrow terminal clamps
        // to leave MIN_DIFF_WIDTH room (here 60 - 24 = 36) instead of
        // returning 0.
        let pref = Preference { preferred: 38 };
        let w = pref.effective(60);
        assert_eq!(w, 60 - MIN_DIFF_WIDTH);
        assert!(w >= MIN_SIDEBAR_WIDTH);
    }

    #[test]
    fn effective_floors_at_min_when_no_diff_room() {
        // Even when the terminal can't leave MIN_DIFF_WIDTH room, the
        // sidebar wins the space and never drops below the floor.
        let pref = Preference { preferred: 10 };
        assert_eq!(pref.effective(20), MIN_SIDEBAR_WIDTH);
    }

    #[test]
    fn effective_uses_preferred_when_it_fits() {
        let pref = Preference { preferred: 38 };
        assert_eq!(pref.effective(200), 38);
    }

    #[test]
    fn effective_clamps_to_min() {
        let pref = Preference { preferred: 1 };
        assert_eq!(pref.effective(200), MIN_SIDEBAR_WIDTH);
    }

    #[test]
    fn effective_clamps_to_leave_diff_room() {
        // A huge preference is capped so the content pane keeps
        // MIN_DIFF_WIDTH.
        let pref = Preference {
            preferred: u16::MAX,
        };
        assert_eq!(pref.effective(100), 100 - MIN_DIFF_WIDTH);
    }

    #[test]
    fn widen_and_narrow_step_by_configured_amount() {
        let mut pref = Preference { preferred: 40 };
        pref.widen();
        assert_eq!(pref.preferred, 40 + SIDEBAR_RESIZE_STEP);
        pref.narrow();
        assert_eq!(pref.preferred, 40);
    }

    #[test]
    fn narrow_floors_at_min() {
        let mut pref = Preference {
            preferred: MIN_SIDEBAR_WIDTH,
        };
        pref.narrow();
        assert_eq!(pref.preferred, MIN_SIDEBAR_WIDTH);
    }

    #[test]
    fn set_from_divider_sets_column_plus_one() {
        let mut pref = Preference { preferred: 40 };
        pref.set_from_divider(50);
        assert_eq!(pref.preferred, 51);
    }

    #[test]
    fn set_from_divider_floors_at_min() {
        let mut pref = Preference { preferred: 40 };
        pref.set_from_divider(2);
        assert_eq!(pref.preferred, MIN_SIDEBAR_WIDTH);
    }

    #[test]
    fn diff_pane_width_subtracts_sidebar_and_borders() {
        // terminal 100, sidebar 24 -> 100 - 24 - 2 = 74.
        assert_eq!(diff_pane_width(24, 100), 74);
    }

    #[test]
    fn diff_pane_width_saturates_to_zero_on_tiny_terminal() {
        assert_eq!(diff_pane_width(40, 10), 0);
    }
}

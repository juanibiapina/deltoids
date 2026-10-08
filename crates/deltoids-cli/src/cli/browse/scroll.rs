//! Mouse-wheel scroll feel for the TUI panes.
//!
//! A single physical wheel tick fans out into a *burst* of wheel events
//! (high-resolution scrolling). Applying one motion per event makes lists jump
//! several items per tick. [`WheelScroll`] counts the burst and emits motion in
//! proportion to how far the user scrolled, identical across panes.
//!
//! This is the one place that owns scroll feel: the quotas live here, so
//! changing how fast the TUI scrolls is a one-file edit. Callers map wheel
//! events onto [`WheelScroll::advance`] and apply the returned step count to
//! the pane's own motion (move a selection, scroll a line).

/// Wheel events that advance a [`ScrollKind::List`] pane's selection by one
/// item. A single physical tick emits several events; dividing the count keeps
/// one tick from jumping many items and makes movement proportional to how much
/// was scrolled. Lower to scroll faster, raise to scroll slower.
const LIST_QUOTA: usize = 3;

/// Wheel events per unit of motion for a [`ScrollKind::Content`] pane. One per
/// event (smooth, one line at a time).
const CONTENT_QUOTA: usize = 1;

/// Direction of a wheel gesture. Decoupled from the event source (e.g.
/// crossterm's `MouseEventKind`) so this module stays testable on its own;
/// callers map their event type onto this at the call site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollDir {
    Up,
    Down,
}

/// How a pane responds to the wheel.
///
/// - [`List`](ScrollKind::List): stepped and slow — one selection move per
///   [`LIST_QUOTA`] events.
/// - [`Discrete`](ScrollKind::Discrete): one selection move per event, for
///   Shift+horizontal wheel events that arrive once per physical tick.
/// - [`Content`](ScrollKind::Content): smooth — one unit of motion per event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollKind {
    List,
    Discrete,
    Content,
}

impl ScrollKind {
    fn quota(self) -> usize {
        match self {
            ScrollKind::List => LIST_QUOTA,
            ScrollKind::Discrete | ScrollKind::Content => CONTENT_QUOTA,
        }
    }
}

/// Translates the burst of wheel events from one physical tick into
/// proportional motion.
///
/// Holds the in-progress gesture (pane key `K`, direction, and scroll kind)
/// and the running event count. Generic over the pane key so each TUI passes
/// its own `Focus` value; only equality is required to detect a gesture change.
#[derive(Debug, Clone)]
pub struct WheelScroll<K> {
    /// Pane, direction, and kind the accumulator is counting. Changing any
    /// of them restarts the count so the next event responds at once.
    gesture: Option<(K, ScrollDir, ScrollKind)>,
    /// Events counted toward the next motion step. One step is emitted each time
    /// it reaches the active pane's quota; the remainder carries over so no
    /// scrolling is lost.
    accum: usize,
}

impl<K> Default for WheelScroll<K> {
    fn default() -> Self {
        Self {
            gesture: None,
            accum: 0,
        }
    }
}

impl<K: Copy + Eq> WheelScroll<K> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of motion steps this wheel event contributes for pane `key`,
    /// given its `kind`. The caller applies that pane's own step (move a
    /// selection once, scroll one line) the returned number of times.
    ///
    /// A change of `key`, `dir`, or `kind` restarts the count primed so the
    /// first event of a new gesture moves immediately. The remainder carries
    /// over between events so movement is proportional at any speed.
    pub fn advance(&mut self, key: K, dir: ScrollDir, kind: ScrollKind) -> usize {
        if self.gesture != Some((key, dir, kind)) {
            self.gesture = Some((key, dir, kind));
            // Prime so the first event of a new gesture moves at once.
            self.accum = kind.quota().saturating_sub(1);
        }
        self.accum += 1;
        let quota = kind.quota().max(1);
        let steps = self.accum / quota;
        self.accum -= steps * quota;
        steps
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Pane {
        A,
        B,
    }

    #[test]
    fn list_burst_yields_one_step_per_quota() {
        let mut wheel = WheelScroll::new();
        // First event of a fresh gesture is primed, so it moves at once.
        assert_eq!(wheel.advance(Pane::A, ScrollDir::Down, ScrollKind::List), 1);
        // The next move takes another full quota of events.
        for _ in 0..LIST_QUOTA - 1 {
            assert_eq!(wheel.advance(Pane::A, ScrollDir::Down, ScrollKind::List), 0);
        }
        assert_eq!(wheel.advance(Pane::A, ScrollDir::Down, ScrollKind::List), 1);
    }

    #[test]
    fn changing_pane_reprimes_immediate_move() {
        let mut wheel = WheelScroll::new();
        assert_eq!(wheel.advance(Pane::A, ScrollDir::Down, ScrollKind::List), 1);
        // Switching pane mid-gesture moves immediately rather than carrying the
        // old count forward.
        assert_eq!(wheel.advance(Pane::B, ScrollDir::Down, ScrollKind::List), 1);
    }

    #[test]
    fn reversing_direction_reprimes_immediate_move() {
        let mut wheel = WheelScroll::new();
        assert_eq!(wheel.advance(Pane::A, ScrollDir::Down, ScrollKind::List), 1);
        assert_eq!(wheel.advance(Pane::A, ScrollDir::Up, ScrollKind::List), 1);
    }

    #[test]
    fn switching_scroll_kind_reprimes_the_selection() {
        let mut wheel = WheelScroll::new();
        assert_eq!(wheel.advance(Pane::A, ScrollDir::Down, ScrollKind::List), 1);
        assert_eq!(wheel.advance(Pane::A, ScrollDir::Down, ScrollKind::List), 0);
        assert_eq!(
            wheel.advance(Pane::A, ScrollDir::Down, ScrollKind::Discrete),
            1
        );
        assert_eq!(
            wheel.advance(Pane::A, ScrollDir::Down, ScrollKind::Discrete),
            1
        );
        assert_eq!(wheel.advance(Pane::A, ScrollDir::Down, ScrollKind::List), 1);
        assert_eq!(wheel.advance(Pane::A, ScrollDir::Down, ScrollKind::List), 0);
    }

    #[test]
    fn content_moves_every_event() {
        let mut wheel = WheelScroll::new();
        for _ in 0..5 {
            assert_eq!(
                wheel.advance(Pane::A, ScrollDir::Down, ScrollKind::Content),
                1
            );
        }
    }

    #[test]
    fn remainder_carries_over_across_quota_boundary() {
        let mut wheel = WheelScroll::new();
        // Prime + count up to one short of two full quotas, then confirm no
        // event is lost: total steps equal the total quotas crossed.
        let mut total = 0;
        for _ in 0..(2 * LIST_QUOTA) {
            total += wheel.advance(Pane::A, ScrollDir::Down, ScrollKind::List);
        }
        // First event primed (1) + one more full quota reached within 2*quota
        // events = 2 steps.
        assert_eq!(total, 2);
    }
}

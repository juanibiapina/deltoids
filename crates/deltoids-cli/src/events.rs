//! Shared input-event draining for the interactive TUIs.

use std::time::Duration;

use crossterm::event::{self, Event};

/// Read every input event already buffered into a single batch, blocking up
/// to `timeout` for the first one. Returns an empty `Vec` on timeout.
///
/// Draining the whole queue before redrawing keeps a burst of repeated
/// events collapsed into a single redraw instead of one redraw per event.
/// Key repeats (holding `j`) and the stream of mouse `Drag` events during a
/// resize both arrive as bursts; processing one per frame would let motion
/// continue after the input stops, as the backlog drains one-per-frame.
pub fn read_event_burst(timeout: Duration) -> Result<Vec<Event>, String> {
    let poll_err = |err| format!("failed to poll input event: {err}");
    let read_err = |err| format!("failed to read input event: {err}");

    if !event::poll(timeout).map_err(poll_err)? {
        return Ok(Vec::new());
    }
    let mut burst = vec![event::read().map_err(read_err)?];
    while event::poll(Duration::ZERO).map_err(poll_err)? {
        burst.push(event::read().map_err(read_err)?);
    }
    Ok(burst)
}

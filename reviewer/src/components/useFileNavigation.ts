import { useEffect, useRef, useCallback } from "react";
import { setPinning } from "../core/pinSignal";

// Pinning a clicked file under the sticky topbar is not a one-shot scroll:
// cards render lazily, so content above the target keeps changing height after
// the jump — sometimes seconds later, when a large diff finishes rendering.
// Native scroll anchoring covers growth fully above the viewport but not the
// boundary card straddling the topbar. So after a click we hold the target with
// a per-frame loop that re-aligns it to the top — until the user scrolls, the
// one signal that they want to leave.
//
// Release is driven by a `scroll` listener (covers wheel, trackpad, touch,
// keyboard, scrollbar) that ignores the loop's own scrolls by comparing against
// the last offset it set; anything else is the user, and ends the hold.

// Read the effective sticky offset. A pin forces the header shown, so
// --sticky-top resolves to the topbar height; fall back if it is unset.
function stickyOffset(): number {
  const style = getComputedStyle(document.documentElement);
  const raw =
    style.getPropertyValue("--sticky-top") || style.getPropertyValue("--topbar-h");
  const n = parseInt(raw, 10);
  return Number.isFinite(n) ? n : 96;
}

export function useFileNavigation() {
  const pending = useRef<number | null>(null);
  const frame = useRef<number | null>(null);
  const lastSetY = useRef(-1);

  const stop = useCallback(() => {
    pending.current = null;
    setPinning(false); // release the forced-shown header
    if (frame.current !== null) {
      cancelAnimationFrame(frame.current);
      frame.current = null;
    }
  }, []);

  const tick = useCallback(() => {
    if (pending.current === null) {
      frame.current = null;
      return;
    }
    const el = document.getElementById(`file-${pending.current}`);
    if (el) {
      // A target with no box is not on screen. That happens when the pinned
      // file is marked viewed while "Hide viewed" is on (the card becomes
      // display:none): its rect is all zeros, and chasing it would scroll to
      // the top every frame. Stop the hold instead; the scroll stays put.
      if (el.getClientRects().length === 0) {
        stop();
        return;
      }
      // Explicit offset (not scrollIntoView + scroll-margin) so it stays exact
      // after any reflow.
      const y = Math.max(0, el.getBoundingClientRect().top + window.scrollY - stickyOffset() - 8);
      lastSetY.current = y;
      // Only scroll when off by more than a pixel, so a settled target does no
      // work frame to frame.
      if (Math.abs(window.scrollY - y) > 1) window.scrollTo(0, y);
    }
    frame.current = requestAnimationFrame(tick);
  }, [stop]);

  const navigateTo = useCallback(
    (index: number) => {
      pending.current = index;
      lastSetY.current = -1; // adopt whatever the loop sets first
      setPinning(true); // hold the header shown while we align the target
      if (frame.current === null) frame.current = requestAnimationFrame(tick);
    },
    [tick],
  );

  useEffect(() => {
    // Any scroll that didn't come from the loop is the user asking to leave.
    const onScroll = () => {
      if (pending.current === null) return;
      if (Math.abs(window.scrollY - lastSetY.current) > 2) stop();
    };
    window.addEventListener("scroll", onScroll, { passive: true });
    return () => {
      window.removeEventListener("scroll", onScroll);
      stop();
    };
  }, [stop]);

  return { navigateTo };
}

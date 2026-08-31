import { useEffect, useRef, useCallback } from "react";

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

function topbarHeight(): number {
  const raw = getComputedStyle(document.documentElement).getPropertyValue("--topbar-h");
  const n = parseInt(raw, 10);
  return Number.isFinite(n) ? n : 96;
}

export function useFileNavigation() {
  const pending = useRef<number | null>(null);
  const frame = useRef<number | null>(null);
  const lastSetY = useRef(-1);

  const tick = useCallback(() => {
    if (pending.current === null) {
      frame.current = null;
      return;
    }
    const el = document.getElementById(`file-${pending.current}`);
    if (el) {
      // Explicit offset (not scrollIntoView + scroll-margin) so it stays exact
      // after any reflow.
      const y = Math.max(0, el.getBoundingClientRect().top + window.scrollY - topbarHeight() - 8);
      lastSetY.current = y;
      // Only scroll when off by more than a pixel, so a settled target does no
      // work frame to frame.
      if (Math.abs(window.scrollY - y) > 1) window.scrollTo(0, y);
    }
    frame.current = requestAnimationFrame(tick);
  }, []);

  const navigateTo = useCallback(
    (index: number) => {
      pending.current = index;
      lastSetY.current = -1; // adopt whatever the loop sets first
      if (frame.current === null) frame.current = requestAnimationFrame(tick);
    },
    [tick],
  );

  useEffect(() => {
    const release = () => {
      pending.current = null;
      if (frame.current !== null) {
        cancelAnimationFrame(frame.current);
        frame.current = null;
      }
    };
    // Any scroll that didn't come from the loop is the user asking to leave.
    const onScroll = () => {
      if (pending.current === null) return;
      if (Math.abs(window.scrollY - lastSetY.current) > 2) release();
    };
    window.addEventListener("scroll", onScroll, { passive: true });
    return () => {
      window.removeEventListener("scroll", onScroll);
      release();
    };
  }, []);

  return { navigateTo };
}

import { useCallback, useEffect, useRef, type RefObject } from "react";
import { isPinning, subscribePinning } from "../core/pinSignal";

export interface CollapseInput {
  prevY: number;
  curY: number;
  // Movement past this many pixels flips the state; smaller moves hold it.
  threshold: number;
  atTop: boolean; // within the top zone -> always show
  focusWithin: boolean; // header holds focus -> always show
  menuOpen: boolean; // settings popover open -> always show
  pinning: boolean; // a file jump is being held -> always show
  current: boolean; // current shown state (true = shown)
}

// Pure quick-return policy: decide whether the header should be shown. Kept
// separate from the DOM so it can be unit-tested without a browser.
export function decideShown(i: CollapseInput): boolean {
  if (i.atTop || i.focusWithin || i.menuOpen || i.pinning) return true;
  const dy = i.curY - i.prevY;
  if (dy > i.threshold) return false; // scrolled down -> hide
  if (dy < -i.threshold) return true; // scrolled up -> show
  return i.current; // within threshold -> unchanged
}

const THRESHOLD = 6;
const TOP_ZONE = 8;

// Wire the pure policy to the page: toggle `chrome-hidden` on <html> from
// scroll, focus, popover, and pin signals. rAF-throttled; clamps overscroll.
export function useChromeCollapse(
  topbarRef: RefObject<HTMLElement | null>,
  menuOpen: boolean,
): void {
  const shown = useRef(true);
  const prevY = useRef(0);
  const raf = useRef<number | null>(null);
  const menuOpenRef = useRef(menuOpen);
  const pinningRef = useRef(isPinning());
  menuOpenRef.current = menuOpen;

  const evaluate = useCallback(() => {
    const curY = Math.max(0, window.scrollY);
    const focusWithin = Boolean(
      topbarRef.current?.contains(document.activeElement),
    );
    const next = decideShown({
      prevY: prevY.current,
      curY,
      threshold: THRESHOLD,
      atTop: curY <= TOP_ZONE,
      focusWithin,
      menuOpen: menuOpenRef.current,
      pinning: pinningRef.current,
      current: shown.current,
    });
    prevY.current = curY;
    if (next !== shown.current) {
      shown.current = next;
      document.documentElement.classList.toggle("chrome-hidden", !next);
    }
  }, [topbarRef]);

  useEffect(() => {
    const onScroll = () => {
      if (raf.current !== null) return;
      raf.current = requestAnimationFrame(() => {
        raf.current = null;
        evaluate();
      });
    };
    window.addEventListener("scroll", onScroll, { passive: true });
    window.addEventListener("focusin", evaluate);
    window.addEventListener("focusout", evaluate);
    const unsub = subscribePinning((value) => {
      pinningRef.current = value;
      evaluate();
    });
    return () => {
      window.removeEventListener("scroll", onScroll);
      window.removeEventListener("focusin", evaluate);
      window.removeEventListener("focusout", evaluate);
      unsub();
      if (raf.current !== null) cancelAnimationFrame(raf.current);
    };
  }, [evaluate]);

  // Opening the popover must reveal the header immediately.
  useEffect(() => {
    evaluate();
  }, [menuOpen, evaluate]);
}

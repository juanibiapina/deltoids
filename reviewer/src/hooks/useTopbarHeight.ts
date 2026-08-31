import { useEffect, type RefObject } from "react";

// Keep --topbar-h in sync with the real topbar, which grows when its form wraps
// onto its own row on narrow screens. Sticky offsets and jump targets read it.
export function useTopbarHeight(ref: RefObject<HTMLElement | null>): void {
  useEffect(() => {
    const el = ref.current;
    if (!el) return;

    const sync = () => {
      const h = Math.round(el.getBoundingClientRect().height);
      document.documentElement.style.setProperty("--topbar-h", `${h}px`);
    };

    const observer = new ResizeObserver(sync);
    observer.observe(el);
    window.addEventListener("resize", sync);
    sync();

    return () => {
      observer.disconnect();
      window.removeEventListener("resize", sync);
    };
  }, [ref]);
}

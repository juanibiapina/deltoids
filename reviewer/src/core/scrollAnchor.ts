// Keeps the text at the top of a scroller still while content above it
// changes size: a file card that finishes loading, a revealed gap, a theme
// re-render, or a hunk that grows its context. Browsers do this natively
// (`overflow-anchor`), but Safari does not, so the scroller turns native
// anchoring off and uses this everywhere.
//
// The anchor is the first row-level element at the top edge of the
// scroller, remembered with its card and a key (`kind + line number`) that
// survives the card's HTML being replaced. A ResizeObserver on the content
// fires after layout and before paint, so the correction lands in the same
// frame as the change.

const STABLE = ".row, .gap, .breadcrumb, .lineno, .file-head, .file-rename, .notice";

interface Anchor {
  el: HTMLElement;
  card: HTMLElement | null;
  key: string | null;
  offset: number;
  cardOffset: number;
}

function rowKey(el: HTMLElement): string | null {
  if (el.classList.contains("file-head")) return "head";
  if (!el.classList.contains("row")) return null;
  const kind = ["added", "removed", "context"].find((k) => el.classList.contains(k));
  const ln = el.querySelector(".ln")?.textContent ?? "";
  return kind ? `${kind}:${ln}` : null;
}

function shown(el: HTMLElement | null): el is HTMLElement {
  return !!el && el.isConnected && el.getClientRects().length > 0;
}

export function attachScrollAnchor(scroller: HTMLElement, content: HTMLElement): () => void {
  if (typeof ResizeObserver === "undefined" || !document.elementsFromPoint) return () => {};
  let anchor: Anchor | null = null;
  const top = () => scroller.getBoundingClientRect().top;

  const pick = () => {
    const rect = scroller.getBoundingClientRect();
    const hits = document.elementsFromPoint(rect.left + rect.width / 3, rect.top + 1);
    for (const hit of hits) {
      if (!scroller.contains(hit)) continue;
      const el = (hit as HTMLElement).closest<HTMLElement>(STABLE);
      if (!el || getComputedStyle(el).position === "sticky") continue;
      const card = el.closest<HTMLElement>(".file");
      anchor = {
        el,
        card,
        key: rowKey(el),
        offset: el.getBoundingClientRect().top - rect.top,
        cardOffset: card ? card.getBoundingClientRect().top - rect.top : 0,
      };
      return;
    }
    anchor = null;
  };

  const find = (a: Anchor): HTMLElement | null => {
    if (shown(a.el)) return a.el;
    if (!a.key || !shown(a.card)) return null;
    for (const el of a.card.querySelectorAll<HTMLElement>(".file-head, .row")) {
      if (rowKey(el) === a.key) return el;
    }
    return null;
  };

  const restore = () => {
    if (!anchor) return;
    const el = find(anchor);
    let delta: number;
    if (el) {
      anchor.el = el;
      delta = el.getBoundingClientRect().top - top() - anchor.offset;
    } else if (shown(anchor.card)) {
      delta = anchor.card.getBoundingClientRect().top - top() - anchor.cardOffset;
    } else {
      anchor = null;
      return;
    }
    if (Math.abs(delta) >= 0.5) scroller.scrollTop += delta;
    pick();
  };

  const resize = new ResizeObserver(restore);
  resize.observe(content);
  scroller.addEventListener("scroll", pick, { passive: true });
  pick();
  return () => {
    resize.disconnect();
    scroller.removeEventListener("scroll", pick);
  };
}

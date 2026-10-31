import type { RefObject } from "react";
import { reshapeHunkAtTop, scrollToHunk } from "../core/hunkNav";

// The phone layout's bottom bar: the file list and the hunk at the reading
// position, within thumb reach.
export function MobileBar({
  scrollerRef,
  onFiles,
}: {
  scrollerRef: RefObject<HTMLElement | null>;
  onFiles: () => void;
}) {
  const act = (work: (scroller: HTMLElement) => void) => () => {
    if (scrollerRef.current) work(scrollerRef.current);
  };
  return (
    <nav className="mobile-bar" aria-label="Review">
      <button type="button" className="tool" onClick={onFiles}>
        Files
      </button>
      <button
        type="button"
        className="tool"
        aria-label="Previous hunk"
        onClick={act((s) => scrollToHunk(s, -1))}
      >
        ↑
      </button>
      <button
        type="button"
        className="tool"
        aria-label="Next hunk"
        onClick={act((s) => scrollToHunk(s, 1))}
      >
        ↓
      </button>
      <button
        type="button"
        className="tool"
        aria-label="Less context"
        onClick={act((s) => reshapeHunkAtTop(s, "shrink"))}
      >
        −
      </button>
      <button
        type="button"
        className="tool"
        aria-label="More context"
        onClick={act((s) => reshapeHunkAtTop(s, "expand"))}
      >
        +
      </button>
    </nav>
  );
}

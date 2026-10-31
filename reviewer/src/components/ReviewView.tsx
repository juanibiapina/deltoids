import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type TouchEvent,
} from "react";
import type { Pr, PrFile } from "../core/github";
import type { PrRef } from "../core/lib";
import type { RenderSession } from "../core/renderSession";
import {
  buildTree,
  displayOrder,
  selectionFiles,
  stepFile,
  stepSelection,
  type Selection,
} from "../core/filetree";
import { createWheelStepper } from "../core/wheel";
import { createSwipeTracker, type SwipeEdges } from "../core/swipe";
import { attachScrollAnchor } from "../core/scrollAnchor";
import { reshapeHunkAtTop } from "../core/hunkNav";
import { useReviewed } from "../hooks/useReviewed";
import { useMediaQuery, WIDE_QUERY } from "../hooks/useMediaQuery";
import { LazyObserverProvider } from "./LazyObserver";
import { FileTree } from "./FileTree";
import { FileCard } from "./FileCard";
import { DiffScrollbar } from "./DiffScrollbar";

export type Layout = "phone" | "desktop";

export interface ReviewData {
  ref: PrRef;
  pr: Pr;
  files: PrFile[];
  session: RenderSession;
}

interface ReviewViewProps {
  data: ReviewData;
  syntaxTheme: string;
  hideViewed: boolean;
  layout?: Layout;
  onNavigate: () => void;
  // Opens the file tree: the drawer on small screens, the full-height sheet
  // on the phone layout.
  onShowTree: () => void;
}

function isTyping(target: EventTarget | null): boolean {
  const el = target as HTMLElement | null;
  return !!el && (el.isContentEditable || /^(INPUT|SELECT|TEXTAREA)$/.test(el.tagName));
}

function selectionKey(selection: Selection): string {
  return selection.kind === "file" ? `file:${selection.index}` : `dir:${selection.id}`;
}

// Where the touched diff's horizontal scroll sits. `.diff` is the only element
// that scrolls sideways (nowrap mode); outside one, or with no overflow, both
// edges hold.
function swipeEdges(target: Element): SwipeEdges {
  const diff = target.closest(".diff");
  if (!diff) return { atLeft: true, atRight: true };
  return {
    atLeft: diff.scrollLeft <= 0,
    atRight: diff.scrollLeft + diff.clientWidth >= diff.scrollWidth - 1,
  };
}

export function ReviewView({
  data,
  syntaxTheme,
  hideViewed,
  layout = "desktop",
  onNavigate,
  onShowTree,
}: ReviewViewProps) {
  const phone = layout === "phone";
  const { ref, files, session } = data;
  const { isReviewed, toggle } = useReviewed(ref, files);
  const small = !useMediaQuery(WIDE_QUERY, true) || phone;

  const tree = useMemo(
    () => buildTree(files.map((f) => ({ filename: f.filename, status: f.status }))),
    [files],
  );
  const order = useMemo(() => displayOrder(tree), [tree]);

  const [selection, setSelection] = useState<Selection>(() => {
    const first =
      order.find((i) => !(hideViewed && isReviewed(files[i]))) ?? order[0] ?? 0;
    return { kind: "file", index: first };
  });
  const shownOrder = useMemo(() => selectionFiles(tree, selection), [tree, selection]);
  const shown = useMemo(() => new Set(shownOrder), [shownOrder]);

  useEffect(() => {
    session.prioritize(shownOrder);
  }, [session, shownOrder]);
  const key = selectionKey(selection);

  const scrollerRef = useRef<HTMLDivElement>(null);
  const contentRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (scrollerRef.current) scrollerRef.current.scrollTop = 0;
  }, [key]);

  useEffect(() => {
    const scroller = scrollerRef.current;
    const content = contentRef.current;
    if (!scroller || !content) return;
    return attachScrollAnchor(scroller, content);
  }, []);

  const isReviewedByIndex = useCallback(
    (index: number) => isReviewed(files[index]),
    [isReviewed, files],
  );
  const isHidden = hideViewed ? isReviewedByIndex : undefined;

  // Small screens open a review on the file tree. Mount only, so a later
  // render never reopens the drawer.
  useEffect(() => {
    if (small) onShowTree();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const collapsedRef = useRef(new Set<string>());
  const handleExpandChange = useCallback((id: string, expanded: boolean) => {
    if (expanded) collapsedRef.current.delete(id);
    else collapsedRef.current.add(id);
  }, []);

  const paneRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const pane = paneRef.current;
    if (!pane) return;
    const stepper = createWheelStepper();
    const onWheel = (e: WheelEvent) => {
      if (!e.ctrlKey && !e.shiftKey) return;
      e.preventDefault();
      const step = stepper.push(e);
      if (step === 0) return;
      setSelection((prev) =>
        stepSelection(tree, prev, step, { collapsed: collapsedRef.current, isHidden }),
      );
    };
    pane.addEventListener("wheel", onWheel, { passive: false });
    return () => pane.removeEventListener("wheel", onWheel);
  }, [tree, isHidden]);

  // z / x grow and shrink the hunk at the reading position, as in the TUI.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.ctrlKey || e.metaKey || e.altKey || isTyping(e.target)) return;
      const scroller = scrollerRef.current;
      if (!scroller || (e.key !== "z" && e.key !== "x")) return;
      if (reshapeHunkAtTop(scroller, e.key === "z" ? "expand" : "shrink")) e.preventDefault();
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, []);

  const handleSelect = useCallback(
    (next: Selection) => {
      onNavigate();
      setSelection(next);
    },
    [onNavigate],
  );

  // Small screens: a sideways swipe over the diff steps between files, and a
  // right swipe on the first file goes back to the tree.
  const swipeRef = useRef(createSwipeTracker());
  const onTouchStart = (e: TouchEvent) => {
    if (!small) return;
    const tracker = swipeRef.current;
    const target = e.target as Element;
    // A second finger (pinch zoom) or the scrollbar track is no swipe.
    if (e.touches.length > 1 || target.closest(".diff-scrollbar")) {
      tracker.cancel();
      return;
    }
    const touch = e.touches[0];
    tracker.start(touch.clientX, touch.clientY, swipeEdges(target));
  };
  const onTouchEnd = (e: TouchEvent) => {
    if (!small) return;
    const touch = e.changedTouches[0];
    if (!touch) return;
    const step = swipeRef.current.end(touch.clientX, touch.clientY);
    if (step === 0) return;
    const next = stepFile(tree, selection, step, isHidden);
    if (step === -1 && next === selection) onShowTree();
    else setSelection(next);
  };
  const onTouchCancel = () => {
    if (small) swipeRef.current.cancel();
  };

  // Small screens: marking the shown file viewed moves on to the next file,
  // or to the tree after the last one.
  const handleToggleReviewed = (index: number) => {
    const wasReviewed = isReviewed(files[index]);
    toggle(files[index]);
    if (wasReviewed || !small) return;
    if (selection.kind !== "file" || selection.index !== index) return;
    const next = stepFile(tree, selection, 1, isHidden);
    if (next === selection) onShowTree();
    else setSelection(next);
  };

  return (
    <LazyObserverProvider rootRef={scrollerRef}>
      <div className="layout">
        <FileTree
          files={files}
          onSelect={handleSelect}
          isReviewed={isReviewedByIndex}
          hideReviewed={hideViewed}
          selection={selection}
          onExpandChange={handleExpandChange}
        />
        <div
          className="pane"
          ref={paneRef}
          onTouchStart={onTouchStart}
          onTouchEnd={onTouchEnd}
          onTouchCancel={onTouchCancel}
        >
          <div className="pane-scroll" ref={scrollerRef}>
            <div ref={contentRef}>
              {order.map((i) => (
                <FileCard
                  key={i}
                  index={i}
                  file={files[i]}
                  session={session}
                  syntaxTheme={syntaxTheme}
                  reviewed={isReviewed(files[i])}
                  onToggleReviewed={() => handleToggleReviewed(i)}
                  hidden={!shown.has(i)}
                  solo={selection.kind === "file"}
                  onOpenFiles={phone ? onShowTree : undefined}
                />
              ))}
            </div>
          </div>
          {!phone && (
            <DiffScrollbar
              scrollerRef={scrollerRef}
              contentRef={contentRef}
              selectionKey={key}
            />
          )}
        </div>
      </div>
    </LazyObserverProvider>
  );
}

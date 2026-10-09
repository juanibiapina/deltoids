import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { Pr, PrFile } from "../core/github";
import type { PrRef } from "../core/lib";
import type { RenderSession } from "../core/renderSession";
import {
  buildTree,
  displayOrder,
  selectionFiles,
  stepSelection,
  type Selection,
} from "../core/filetree";
import { createWheelStepper } from "../core/wheel";
import { useReviewed } from "../hooks/useReviewed";
import { LazyObserverProvider } from "./LazyObserver";
import { FileTree } from "./FileTree";
import { FileCard } from "./FileCard";
import { DiffScrollbar } from "./DiffScrollbar";

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
  onNavigate: () => void;
}

function selectionKey(selection: Selection): string {
  return selection.kind === "file" ? `file:${selection.index}` : `dir:${selection.id}`;
}

export function ReviewView({
  data,
  syntaxTheme,
  hideViewed,
  onNavigate,
}: ReviewViewProps) {
  const { ref, files, session } = data;
  const { isReviewed, toggle } = useReviewed(ref, files);

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

  const isReviewedByIndex = useCallback(
    (index: number) => isReviewed(files[index]),
    [isReviewed, files],
  );

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
    const isHidden = hideViewed ? isReviewedByIndex : undefined;
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
  }, [tree, hideViewed, isReviewedByIndex]);

  const handleSelect = useCallback(
    (next: Selection) => {
      onNavigate();
      setSelection(next);
    },
    [onNavigate],
  );

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
        <div className="pane" ref={paneRef}>
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
                  onToggleReviewed={() => toggle(files[i])}
                  hidden={!shown.has(i)}
                  solo={selection.kind === "file"}
                />
              ))}
            </div>
          </div>
          <DiffScrollbar
            scrollerRef={scrollerRef}
            contentRef={contentRef}
            selectionKey={key}
          />
        </div>
      </div>
    </LazyObserverProvider>
  );
}

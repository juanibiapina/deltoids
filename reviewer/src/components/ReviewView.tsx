import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { Engine } from "../core/engine";
import type { Pr, PrFile } from "../core/github";
import type { PrRef } from "../core/lib";
import {
  buildTree,
  displayOrder,
  selectionFiles,
  type Selection,
} from "../core/filetree";
import { useReviewed } from "../hooks/useReviewed";
import { LazyObserverProvider } from "./LazyObserver";
import { FileTree } from "./FileTree";
import { FileCard } from "./FileCard";
import { DiffScrollbar } from "./DiffScrollbar";

export interface ReviewData {
  ref: PrRef;
  pr: Pr;
  files: PrFile[];
  engine: Engine;
  baseSha: string;
  headSha: string;
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
  const { ref, files, engine, baseSha, headSha } = data;
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
  const shown = useMemo(() => new Set(selectionFiles(tree, selection)), [tree, selection]);
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
        />
        <div className="pane">
          <div className="pane-scroll" ref={scrollerRef}>
            <div ref={contentRef}>
              {order.map((i) => (
                <FileCard
                  key={i}
                  index={i}
                  file={files[i]}
                  engine={engine}
                  repoRef={ref}
                  baseSha={baseSha}
                  headSha={headSha}
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

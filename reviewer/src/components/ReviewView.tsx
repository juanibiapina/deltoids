import { useCallback, useEffect, useRef, useState } from "react";
import type { Engine } from "../core/engine";
import type { Pr, PrFile } from "../core/github";
import type { PrRef } from "../core/lib";
import { pickActiveIndex } from "../core/activeFile";
import { useReviewed } from "../hooks/useReviewed";
import { LazyObserverProvider } from "./LazyObserver";
import { FileTree } from "./FileTree";
import { FileCard } from "./FileCard";
import { useFileNavigation } from "./useFileNavigation";

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
  onProgress: (loaded: number, total: number) => void;
}

export function ReviewView({
  data,
  syntaxTheme,
  hideViewed,
  onNavigate,
  onProgress,
}: ReviewViewProps) {
  const { ref, pr, files, engine, baseSha, headSha } = data;
  const loaded = useRef(0);
  const { navigateTo } = useFileNavigation();
  const { isReviewed, toggle, count, clear } = useReviewed(ref, files);

  // Scrollspy: highlight the file currently at the top of the diff column in
  // the tree. A continuous IntersectionObserver (separate from the one-shot
  // lazy-load observer) watches every card against a thin band under the
  // topbar; the topmost card crossing it is active. jsdom has no
  // IntersectionObserver, so this stays inert in tests.
  const columnRef = useRef<HTMLDivElement>(null);
  const [activeIndex, setActiveIndex] = useState<number | null>(null);
  const activeRef = useRef<number | null>(null);
  activeRef.current = activeIndex;

  useEffect(() => {
    if (typeof IntersectionObserver === "undefined") return;
    const column = columnRef.current;
    if (!column) return;

    const build = () => {
      const intersecting = new Set<number>();
      // Detection band: from just below the sticky topbar down through the
      // upper third of the viewport. Starting at the topbar height (not 0) is
      // essential — otherwise the band sits *behind* the topbar and tracks the
      // card sliding up out of view instead of the one at the readable top,
      // lagging the highlight by a file or two. The band's height only needs to
      // be tall enough that some card edge always falls inside it; the topmost
      // (smallest) intersecting index is the active file, so the tail does not
      // make the pick eager.
      const topbar =
        parseInt(
          getComputedStyle(document.documentElement).getPropertyValue("--topbar-h"),
          10,
        ) || 60;
      const observer = new IntersectionObserver(
        (entries) => {
          for (const entry of entries) {
            const idx = Number((entry.target as HTMLElement).id.slice("file-".length));
            if (!Number.isFinite(idx)) continue;
            if (entry.isIntersecting) intersecting.add(idx);
            else intersecting.delete(idx);
          }
          const next = pickActiveIndex(intersecting, activeRef.current);
          if (next !== activeRef.current) setActiveIndex(next);
        },
        { rootMargin: `-${topbar}px 0px -70% 0px` },
      );
      column
        .querySelectorAll<HTMLElement>('section.file[id^="file-"]')
        .forEach((el) => observer.observe(el));
      return observer;
    };

    let observer = build();
    // The topbar height (baked into rootMargin) can change when the layout
    // reflows at a breakpoint; rebuild so the band stays under the bar.
    const onResize = () => {
      observer.disconnect();
      observer = build();
    };
    window.addEventListener("resize", onResize);
    return () => {
      window.removeEventListener("resize", onResize);
      observer.disconnect();
    };
  }, [files]);

  // Stable index-keyed lookup for the sidebar (both dimming and pruning).
  const isReviewedByIndex = useCallback(
    (index: number) => isReviewed(files[index]),
    [isReviewed, files],
  );

  const handleLoaded = useCallback(() => {
    loaded.current += 1;
    onProgress(loaded.current, files.length);
  }, [files.length, onProgress]);

  const handleFileSelect = useCallback(
    (index: number) => {
      onNavigate(); // close the mobile drawer
      navigateTo(index);
    },
    [onNavigate, navigateTo],
  );

  const capped = files.length >= 3000 ? " (first 3000)" : "";

  return (
    <LazyObserverProvider>
      <div className="layout">
        <FileTree
          files={files}
          onFileSelect={handleFileSelect}
          isReviewed={isReviewedByIndex}
          hideReviewed={hideViewed}
          activeIndex={activeIndex}
        />
        <div className="column" ref={columnRef}>
          <div className="pr-meta">
            <h1>
              #{pr.number} · {pr.title}
            </h1>
            <div className="sub">
              {ref.owner}/{ref.repo} · {files.length} files{capped} · +
              {pr.additions} −{pr.deletions}
            </div>
            {count > 0 && (
              <div className="review-progress">
                <span>
                  {count} of {files.length} reviewed
                </span>
                <span className="review-bar" aria-hidden="true">
                  <span
                    style={{ width: `${(count / files.length) * 100}%` }}
                  ></span>
                </span>
                <button type="button" className="review-clear" onClick={clear}>
                  Clear
                </button>
              </div>
            )}
          </div>
          {files.map((file, i) => (
            <FileCard
              key={i}
              index={i}
              file={file}
              engine={engine}
              repoRef={ref}
              baseSha={baseSha}
              headSha={headSha}
              syntaxTheme={syntaxTheme}
              reviewed={isReviewed(file)}
              onToggleReviewed={() => toggle(file)}
              onLoaded={handleLoaded}
            />
          ))}
        </div>
      </div>
    </LazyObserverProvider>
  );
}

import { useCallback, useRef } from "react";
import type { Engine } from "../core/engine";
import type { Pr, PrFile } from "../core/github";
import type { PrRef } from "../core/lib";
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
        />
        <div className="column">
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

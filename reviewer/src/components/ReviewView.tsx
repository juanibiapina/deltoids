import { useCallback, useRef } from "react";
import type { Engine } from "../core/engine";
import type { Pr, PrFile } from "../core/github";
import type { PrRef } from "../core/lib";
import { LazyObserverProvider } from "./LazyObserver";
import { Sidebar } from "./Sidebar";
import { FileCard } from "./FileCard";

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
  onNavigate: () => void;
  onProgress: (loaded: number, total: number) => void;
}

export function ReviewView({ data, onNavigate, onProgress }: ReviewViewProps) {
  const { ref, pr, files, engine, baseSha, headSha } = data;
  const loaded = useRef(0);

  const handleLoaded = useCallback(() => {
    loaded.current += 1;
    onProgress(loaded.current, files.length);
  }, [files.length, onProgress]);

  const capped = files.length >= 3000 ? " (first 3000)" : "";

  return (
    <LazyObserverProvider>
      <div className="layout">
        <Sidebar files={files} onNavigate={onNavigate} />
        <div className="column">
          <div className="pr-meta">
            <h1>
              #{pr.number} · {pr.title}
            </h1>
            <div className="sub">
              {ref.owner}/{ref.repo} · {files.length} files{capped} · +
              {pr.additions} −{pr.deletions}
            </div>
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
              onLoaded={handleLoaded}
            />
          ))}
        </div>
      </div>
    </LazyObserverProvider>
  );
}

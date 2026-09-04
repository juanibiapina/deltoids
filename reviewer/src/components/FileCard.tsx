import { useEffect, useRef, useState } from "react";
import { badgeClass } from "../core/lib";
import { estimateCardHeight } from "../core/cardHeight";
import { loadSides, renderSides, type PrFile, type Sides } from "../core/github";
import type { Engine } from "../core/engine";
import type { PrRef } from "../core/lib";
import { useLazy } from "./LazyObserver";

interface FileCardProps {
  index: number;
  file: PrFile;
  engine: Engine;
  repoRef: PrRef;
  baseSha: string;
  headSha: string;
  syntaxTheme: string;
  reviewed: boolean;
  onToggleReviewed: () => void;
  onLoaded: () => void;
}

type Body =
  | { kind: "pending" }
  | { kind: "html"; html: string }
  | { kind: "notice"; text: string };

export function FileCard({
  index,
  file,
  engine,
  repoRef,
  baseSha,
  headSha,
  syntaxTheme,
  reviewed,
  onToggleReviewed,
  onLoaded,
}: FileCardProps) {
  const ref = useRef<HTMLElement>(null);
  const diffRef = useRef<HTMLDivElement>(null);
  const lazy = useLazy();
  const [body, setBody] = useState<Body>({ kind: "pending" });
  // New-file start lines of the gap dividers the user has expanded. Kept in
  // state (not the DOM) so an expansion survives a theme re-render, which
  // replaces the injected HTML.
  const [expandedGaps, setExpandedGaps] = useState<Set<number>>(new Set());
  const loadedOnce = useRef(false);
  // Cached fetched content: `undefined` until loaded, `null` for a binary
  // file, otherwise the sides to render. Re-rendering on a theme switch reads
  // this instead of re-fetching from GitHub.
  const sidesRef = useRef<Sides | null | undefined>(undefined);
  // Latest theme, read by both the initial load and the theme-change effect so
  // whichever fires renders with the current selection.
  const themeRef = useRef(syntaxTheme);
  themeRef.current = syntaxTheme;

  // Render cached sides (or a notice) into the body with the current theme.
  const renderInto = (sides: Sides | null) => {
    if (sides === null) {
      setBody({ kind: "notice", text: "Binary file not shown." });
      return;
    }
    const html = renderSides(engine, sides, themeRef.current);
    setBody(
      html
        ? { kind: "html", html }
        : { kind: "notice", text: "No textual changes." },
    );
  };

  useEffect(() => {
    const el = ref.current;
    if (!el) return;

    let cancelled = false;
    const load = async () => {
      if (loadedOnce.current) return;
      loadedOnce.current = true;
      try {
        const sides = await loadSides(repoRef, file, baseSha, headSha);
        if (cancelled) return;
        sidesRef.current = sides;
        renderInto(sides);
      } catch (err) {
        if (cancelled) return;
        const message = err instanceof Error ? err.message : String(err);
        setBody({ kind: "notice", text: `Could not load: ${message}` });
      } finally {
        if (!cancelled) onLoaded();
      }
    };

    lazy.observe(el, load);
    return () => {
      cancelled = true;
      lazy.unobserve(el);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Re-render from cached sides when the syntax theme changes — no re-fetch.
  // Skips files not loaded yet (their first render already uses the current
  // theme) and binary files (nothing to recolor).
  useEffect(() => {
    if (sidesRef.current === undefined || sidesRef.current === null) return;
    renderInto(sidesRef.current);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [syntaxTheme]);

  // Record a clicked gap divider so the expansion effect reveals it.
  const onDiffClick = (event: React.MouseEvent<HTMLDivElement>) => {
    const gap = (event.target as HTMLElement).closest?.(".gap") as
      | HTMLElement
      | null;
    if (!gap) return;
    const start = Number(gap.dataset.gapNewStart);
    if (!Number.isFinite(start)) return;
    setExpandedGaps((prev) => {
      if (prev.has(start)) return prev;
      const next = new Set(prev);
      next.add(start);
      return next;
    });
  };

  // After each diff render (or expansion change), reveal every expanded gap by
  // rendering its new-file range as context rows and injecting them where the
  // divider stood. A theme re-render replaces the HTML and rebuilds the `.gap`
  // nodes, so this runs again and re-applies with the current theme.
  useEffect(() => {
    const root = diffRef.current;
    const sides = sidesRef.current;
    if (!root || body.kind !== "html" || !sides) return;
    root.querySelectorAll<HTMLElement>(".gap").forEach((gap) => {
      const start = Number(gap.dataset.gapNewStart);
      const end = Number(gap.dataset.gapNewEnd);
      if (!expandedGaps.has(start) || !Number.isFinite(end)) return;
      // The hunk right after the gap now continues directly from the revealed
      // lines, so its header (breadcrumb / line number) and top seam become
      // redundant — mark it "joined" to fold them away.
      const nextHunk = gap.nextElementSibling;
      const rows = engine.renderContext(
        sides.after,
        sides.path,
        start,
        end,
        themeRef.current,
      );
      const template = document.createElement("template");
      template.innerHTML = rows;
      gap.after(template.content);
      gap.remove();
      if (nextHunk?.classList.contains("hunk")) {
        nextHunk.classList.add("joined");
      }
    });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [body, expandedGaps]);

  const badge = badgeClass(file.status);
  const label =
    file.status === "renamed"
      ? `${file.previous_filename} → ${file.filename}`
      : file.filename;

  // While pending, reserve the estimated height so cards loading above this one
  // barely shift the page (keeps jump-to-file accurate).
  const reserve =
    body.kind === "pending" && !reviewed
      ? { minHeight: estimateCardHeight(file.additions, file.deletions) }
      : undefined;

  return (
    <section
      className={reviewed ? "file reviewed" : "file"}
      id={`file-${index}`}
      ref={ref}
      style={reserve}
    >
      <div className="file-head">
        <span className={`badge ${badge}`}>{file.status}</span>
        <span className="path">{label}</span>
        <label className="review-toggle">
          <input
            type="checkbox"
            checked={reviewed}
            onChange={onToggleReviewed}
          />
          Viewed
        </label>
      </div>
      {body.kind === "html" ? (
        <div
          className="diff"
          ref={diffRef}
          onClick={onDiffClick}
          dangerouslySetInnerHTML={{ __html: body.html }}
        />
      ) : (
        <div className="diff">
          {body.kind === "pending" ? (
            <div className="skeleton" aria-hidden="true">
              <div className="bar"></div>
              <div className="bar"></div>
              <div className="bar"></div>
              <div className="bar"></div>
            </div>
          ) : (
            <div className="notice">{body.text}</div>
          )}
        </div>
      )}
    </section>
  );
}

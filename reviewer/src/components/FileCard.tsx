import { useEffect, useRef, useState } from "react";
import { renderSides, type PrFile, type Sides } from "../core/github";
import type { Engine } from "../core/engine";
import type { SidesLoader } from "../core/sidesLoader";
import { whenIdle } from "../core/idle";
import { useLazy } from "./LazyObserver";

interface FileCardProps {
  index: number;
  file: PrFile;
  engine: Engine;
  loader: SidesLoader;
  syntaxTheme: string;
  reviewed: boolean;
  onToggleReviewed: () => void;
  hidden?: boolean;
  solo?: boolean;
}

type Body =
  | { kind: "pending" }
  | { kind: "html"; html: string }
  | { kind: "notice"; text: string };

export function FileCard({
  index,
  file,
  engine,
  loader,
  syntaxTheme,
  reviewed,
  onToggleReviewed,
  hidden = false,
  solo = false,
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
  const renderedTheme = useRef<string | null>(null);
  // The loader's resolved sides, held so a theme switch or gap expansion can
  // render synchronously: `undefined` until loaded, `null` for a binary file.
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
    renderedTheme.current = themeRef.current;
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
    const show = (sides: Sides | null) => {
      if (cancelled || sidesRef.current !== undefined) return;
      sidesRef.current = sides;
      renderInto(sides);
    };

    let cancelIdle = () => {};
    void loader.whenFetched(index).then((sides) => {
      if (cancelled || sidesRef.current !== undefined) return;
      cancelIdle = whenIdle(() => show(sides));
    });

    const load = async () => {
      if (loadedOnce.current) return;
      loadedOnce.current = true;
      try {
        show(await loader.load(index));
      } catch (err) {
        if (cancelled) return;
        const message = err instanceof Error ? err.message : String(err);
        setBody({ kind: "notice", text: `Could not load: ${message}` });
      }
    };

    lazy.observe(el, load);
    return () => {
      cancelled = true;
      cancelIdle();
      lazy.unobserve(el);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Re-render from cached sides when the syntax theme changes — no re-fetch.
  // A shown card re-renders at once; a hidden one waits for an idle slot, or
  // renders at once if it is shown first.
  useEffect(() => {
    const sides = sidesRef.current;
    if (!sides || renderedTheme.current === syntaxTheme) return;
    if (!hidden) {
      renderInto(sides);
      return;
    }
    return whenIdle(() => renderInto(sides));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [syntaxTheme, hidden]);

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

  const className = ["file", reviewed && "reviewed", solo && "solo"]
    .filter(Boolean)
    .join(" ");

  return (
    <section className={className} id={`file-${index}`} ref={ref} hidden={hidden}>
      <div className="file-head">
        <span className="path">{file.filename}</span>
        <label className="review-toggle">
          <input
            type="checkbox"
            checked={reviewed}
            onChange={onToggleReviewed}
          />
          Viewed
        </label>
      </div>
      {file.status === "renamed" && file.previous_filename && (
        <div className="file-rename">
          renamed: {file.previous_filename} ⟶ {file.filename}
        </div>
      )}
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

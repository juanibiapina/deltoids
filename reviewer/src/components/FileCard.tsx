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
  onLoaded,
}: FileCardProps) {
  const ref = useRef<HTMLElement>(null);
  const lazy = useLazy();
  const [body, setBody] = useState<Body>({ kind: "pending" });
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

  const badge = badgeClass(file.status);
  const label =
    file.status === "renamed"
      ? `${file.previous_filename} → ${file.filename}`
      : file.filename;

  // While pending, reserve the estimated height so cards loading above this one
  // barely shift the page (keeps jump-to-file accurate).
  const reserve =
    body.kind === "pending"
      ? { minHeight: estimateCardHeight(file.additions, file.deletions) }
      : undefined;

  return (
    <section className="file" id={`file-${index}`} ref={ref} style={reserve}>
      <div className="file-head">
        <span className={`badge ${badge}`}>{file.status}</span>
        <span className="path">{label}</span>
      </div>
      {body.kind === "html" ? (
        <div className="diff" dangerouslySetInnerHTML={{ __html: body.html }} />
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

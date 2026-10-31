import { useEffect, useRef, useState } from "react";
import type { PrFile } from "../core/github";
import type { RenderSession, Rendered } from "../core/renderSession";
import { whenIdle } from "../core/idle";
import { useLazy } from "./LazyObserver";
import { RESHAPE_EVENT, type ReshapeDetail } from "../core/hunkNav";

interface FileCardProps {
  index: number;
  file: PrFile;
  session: RenderSession;
  syntaxTheme: string;
  reviewed: boolean;
  onToggleReviewed: () => void;
  hidden?: boolean;
  solo?: boolean;
  // When set, the path opens the file list (phone layout).
  onOpenFiles?: () => void;
}

function ChevronDown() {
  return (
    <svg className="path-chevron" viewBox="0 0 16 16" width="14" height="14" aria-hidden="true">
      <path
        d="M4 6l4 4 4-4"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.6"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  );
}

type Body =
  | { kind: "pending" }
  | { kind: "html"; html: string; theme: string }
  | { kind: "notice"; text: string; theme?: string };

function bodyFor(rendered: Rendered, theme: string): Body {
  switch (rendered.kind) {
    case "html":
      return { kind: "html", html: rendered.html, theme };
    case "binary":
      return { kind: "notice", text: "Binary file not shown.", theme };
    case "empty":
      return { kind: "notice", text: "No textual changes.", theme };
  }
}

export function FileCard({
  index,
  file,
  session,
  syntaxTheme,
  reviewed,
  onToggleReviewed,
  hidden = false,
  solo = false,
  onOpenFiles,
}: FileCardProps) {
  const ref = useRef<HTMLElement>(null);
  const diffRef = useRef<HTMLDivElement>(null);
  const lazy = useLazy();
  const [body, setBody] = useState<Body>({ kind: "pending" });
  // New-file start lines of the gap dividers the user has expanded. Kept in
  // state (not the DOM) so an expansion survives a theme re-render, which
  // replaces the injected HTML.
  const [expandedGaps, setExpandedGaps] = useState<Set<number>>(new Set());
  // Set once the card nears the viewport; from then on a shown card renders
  // urgently instead of in the background.
  const [seen, setSeen] = useState(false);
  const shownTheme = useRef<string | null>(null);
  // Gap nodes with a context render in flight, so a re-run of the expansion
  // effect does not request them twice.
  const pendingGaps = useRef(new WeakSet<Element>());

  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    lazy.observe(el, () => setSeen(true));
    return () => lazy.unobserve(el);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Render the current theme. A seen, shown card asks the worker for an urgent
  // render and paints the result at once. Otherwise the render runs in the
  // background and its HTML lands in an idle slot, so hidden cards are ready
  // before they are selected. A result for a theme that is no longer current
  // is dropped by the cleanup.
  useEffect(() => {
    if (shownTheme.current === syntaxTheme) return;
    const urgent = seen && !hidden;
    let cancelled = false;
    let cancelIdle = () => {};
    const apply = (next: Body) => {
      shownTheme.current = syntaxTheme;
      setBody(next);
    };
    session.render(index, syntaxTheme, urgent ? "now" : "background").then(
      (rendered) => {
        if (cancelled) return;
        const next = bodyFor(rendered, syntaxTheme);
        if (urgent) apply(next);
        else cancelIdle = whenIdle(() => apply(next));
      },
      (err) => {
        if (cancelled || !urgent) return;
        const message = err instanceof Error ? err.message : String(err);
        setBody({ kind: "notice", text: `Could not load: ${message}` });
      },
    );
    return () => {
      cancelled = true;
      cancelIdle();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [syntaxTheme, hidden, seen]);

  // Grow or shrink one hunk's context in the engine and show the result.
  const reshape = (op: "expand" | "shrink", hunk: number) => {
    const theme = syntaxTheme;
    void session.reshape(index, op, hunk, theme).then((rendered) => {
      if (!rendered) return;
      shownTheme.current = theme;
      setBody(bodyFor(rendered, theme));
    });
  };

  // `z`/`x` reach a hunk as a RESHAPE_EVENT (core/hunkNav.ts).
  const reshapeRef = useRef(reshape);
  reshapeRef.current = reshape;
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const onReshape = (event: Event) => {
      const { op, hunk } = (event as CustomEvent<ReshapeDetail>).detail;
      reshapeRef.current(op, hunk);
    };
    el.addEventListener(RESHAPE_EVENT, onReshape);
    return () => el.removeEventListener(RESHAPE_EVENT, onReshape);
  }, []);

  // A hunk's scope header grows the hunk to its enclosing scope; a gap divider
  // is recorded so the expansion effect reveals it.
  const onDiffClick = (event: React.MouseEvent<HTMLDivElement>) => {
    const target = event.target as HTMLElement;
    const header = target.closest?.(".hunk > .breadcrumb, .hunk > .lineno");
    if (header) {
      if (window.getSelection()?.toString()) return;
      const hunk = Number(header.closest<HTMLElement>(".hunk")?.dataset.hunk);
      if (Number.isFinite(hunk)) reshape("expand", hunk);
      return;
    }
    const gap = target.closest?.(".gap") as HTMLElement | null;
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
  // nodes, so this runs again and re-applies with the current theme; rows for
  // a replaced node are dropped.
  useEffect(() => {
    const root = diffRef.current;
    if (!root || body.kind !== "html") return;
    root.querySelectorAll<HTMLElement>(".gap").forEach((gap) => {
      const start = Number(gap.dataset.gapNewStart);
      const end = Number(gap.dataset.gapNewEnd);
      if (!expandedGaps.has(start) || !Number.isFinite(end)) return;
      if (pendingGaps.current.has(gap)) return;
      pendingGaps.current.add(gap);
      void session.renderContext(index, start, end, body.theme).then((rows) => {
        if (!gap.isConnected) return;
        // The hunk right after the gap now continues directly from the
        // revealed lines, so its header (breadcrumb / line number) and top
        // seam become redundant — mark it "joined" to fold them away.
        const nextHunk = gap.nextElementSibling;
        const template = document.createElement("template");
        template.innerHTML = rows;
        gap.after(template.content);
        gap.remove();
        if (nextHunk?.classList.contains("hunk")) {
          nextHunk.classList.add("joined");
        }
      });
    });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [body, expandedGaps]);

  const className = ["file", reviewed && "reviewed", solo && "solo"]
    .filter(Boolean)
    .join(" ");

  return (
    <section className={className} id={`file-${index}`} ref={ref} hidden={hidden}>
      <div className="file-head">
        {onOpenFiles ? (
          <button
            type="button"
            className="path path-button"
            aria-label={`${file.filename}, show all files`}
            onClick={onOpenFiles}
          >
            <span className="path-text">{file.filename}</span>
            <ChevronDown />
          </button>
        ) : (
          <span className="path">{file.filename}</span>
        )}
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

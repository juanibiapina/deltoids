import { useEffect, useRef } from "react";
import {
  changeCells,
  scrollForThumbTop,
  thumb,
  CELL_PX,
  type ChangeKind,
  type Geometry,
  type Span,
} from "../core/overview";

const MARKS_PX = 7;

function geometry(scroller: HTMLElement): Geometry {
  return { contentHeight: scroller.scrollHeight, track: scroller.clientHeight };
}

function measureSpans(scroller: HTMLElement): Span[] {
  const offset = scroller.scrollTop - scroller.getBoundingClientRect().top;
  const spans: Span[] = [];
  scroller.querySelectorAll<HTMLElement>(".row.added, .row.removed").forEach((row) => {
    const rect = row.getBoundingClientRect();
    if (rect.height === 0) return;
    const kind: ChangeKind = row.classList.contains("added") ? "added" : "removed";
    const top = rect.top + offset;
    const last = spans[spans.length - 1];
    if (last && last.kind === kind && top <= last.top + last.height + 1) {
      last.height = Math.max(last.height, top + rect.height - last.top);
    } else {
      spans.push({ top, height: rect.height, kind });
    }
  });
  return spans;
}

function draw(canvas: HTMLCanvasElement, spans: Span[], g: Geometry): void {
  const dpr = window.devicePixelRatio || 1;
  canvas.style.height = `${g.track}px`;
  canvas.width = Math.round(MARKS_PX * dpr);
  canvas.height = Math.round(g.track * dpr);
  const ctx = canvas.getContext("2d");
  if (!ctx) return;
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  ctx.clearRect(0, 0, MARKS_PX, g.track);
  const style = getComputedStyle(canvas);
  const added = style.getPropertyValue("--added-gutter").trim();
  const removed = style.getPropertyValue("--removed-gutter").trim();
  changeCells(spans, g).forEach((cell, i) => {
    const y = i * CELL_PX;
    if (cell.added && cell.removed) {
      ctx.fillStyle = added;
      ctx.fillRect(0, y, MARKS_PX, CELL_PX / 2);
      ctx.fillStyle = removed;
      ctx.fillRect(0, y + CELL_PX / 2, MARKS_PX, CELL_PX / 2);
    } else if (cell.added || cell.removed) {
      ctx.fillStyle = cell.added ? added : removed;
      ctx.fillRect(0, y, MARKS_PX, CELL_PX);
    }
  });
}

export function DiffScrollbar({
  scrollerRef,
  contentRef,
  selectionKey,
}: {
  scrollerRef: React.RefObject<HTMLElement | null>;
  contentRef: React.RefObject<HTMLElement | null>;
  selectionKey: string;
}) {
  const trackRef = useRef<HTMLDivElement>(null);
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const thumbRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const scroller = scrollerRef.current;
    const content = contentRef.current;
    const track = trackRef.current;
    const canvas = canvasRef.current;
    const thumbEl = thumbRef.current;
    if (!scroller || !content || !track || !canvas || !thumbEl) return;
    if (typeof ResizeObserver === "undefined") return;

    const placeThumb = () => {
      const t = thumb(scroller.scrollTop, geometry(scroller));
      thumbEl.hidden = t === null;
      if (t) {
        thumbEl.style.top = `${t.top}px`;
        thumbEl.style.height = `${t.height}px`;
      }
    };

    let frame: number | null = null;
    const measure = () => {
      if (frame !== null) return;
      frame = requestAnimationFrame(() => {
        frame = null;
        draw(canvas, measureSpans(scroller), geometry(scroller));
        placeThumb();
      });
    };

    const resize = new ResizeObserver(measure);
    resize.observe(scroller);
    resize.observe(content);
    const themeChange = new MutationObserver(measure);
    themeChange.observe(document.documentElement, {
      attributes: true,
      attributeFilter: ["data-theme"],
    });
    scroller.addEventListener("scroll", placeThumb, { passive: true });
    measure();

    const scrollTo = (thumbTop: number) => {
      scroller.scrollTop = scrollForThumbTop(thumbTop, geometry(scroller));
    };
    let grab: number | null = null;
    const trackY = (e: PointerEvent) => e.clientY - track.getBoundingClientRect().top;
    const onDown = (e: PointerEvent) => {
      const t = thumb(scroller.scrollTop, geometry(scroller));
      if (!t || e.button !== 0) return;
      const y = trackY(e);
      grab = y >= t.top && y <= t.top + t.height ? y - t.top : t.height / 2;
      track.setPointerCapture(e.pointerId);
      scrollTo(y - grab);
      e.preventDefault();
    };
    const onMove = (e: PointerEvent) => {
      if (grab !== null) scrollTo(trackY(e) - grab);
    };
    const onUp = (e: PointerEvent) => {
      grab = null;
      if (track.hasPointerCapture(e.pointerId)) track.releasePointerCapture(e.pointerId);
    };
    track.addEventListener("pointerdown", onDown);
    track.addEventListener("pointermove", onMove);
    track.addEventListener("pointerup", onUp);
    track.addEventListener("pointercancel", onUp);

    return () => {
      if (frame !== null) cancelAnimationFrame(frame);
      resize.disconnect();
      themeChange.disconnect();
      scroller.removeEventListener("scroll", placeThumb);
      track.removeEventListener("pointerdown", onDown);
      track.removeEventListener("pointermove", onMove);
      track.removeEventListener("pointerup", onUp);
      track.removeEventListener("pointercancel", onUp);
    };
  }, [scrollerRef, contentRef, selectionKey]);

  return (
    <div className="diff-scrollbar" ref={trackRef} aria-hidden="true">
      <canvas ref={canvasRef} />
      <div className="diff-scrollbar-thumb" ref={thumbRef} hidden />
    </div>
  );
}

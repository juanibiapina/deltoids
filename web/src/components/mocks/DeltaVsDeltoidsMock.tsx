import * as React from "react";
import { Screenshot } from "./Screenshot";

/**
 * Side-by-side comparison: a default `git diff | delta` (three lines of
 * context) next to the same diff piped through `deltoids` (entire
 * enclosing function expanded as context). Real screenshots taken from
 * the Astro landing page.
 *
 * The deltoids screenshot is intentionally taller — that height
 * difference *is* the pitch.
 */
export function DeltaVsDeltoidsMock() {
  return (
    <div className="grid items-start gap-4 sm:grid-cols-2">
      <figure>
        <figcaption className="mb-2 font-mono text-xs uppercase tracking-[0.14em] text-fg-dim">
          Default
        </figcaption>
        <Screenshot
          src="/screenshots/delta.png"
          alt="Default git diff piped through delta: a hunk header and three lines of context above and below the change. The reader has to scroll up to find which function this is in."
          width={4353}
          height={1776}
        />
      </figure>
      <figure>
        <figcaption className="mb-2 font-mono text-xs uppercase tracking-[0.14em] text-accent">
          deltoids
        </figcaption>
        <Screenshot
          src="/screenshots/deltoids.png"
          alt="Same diff piped through deltoids: the entire enclosing function is included as context, from signature to closing brace, with a cyan boxed scope breadcrumb above the hunk."
          width={4353}
          height={4678}
        />
      </figure>
    </div>
  );
}

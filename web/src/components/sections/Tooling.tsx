import * as React from "react";
import { Reveal } from "@/components/Reveal";
import { GitHubIcon } from "@/components/icons/GitHubIcon";

/**
 * "Plugs into your tools" — replacing the traditional logo cloud since the
 * project has no users to advertise yet. Logos are inline SVG/wordmarks
 * rendered as faint, monochrome chips.
 */
const TOOLS: { name: string; svg: React.ReactNode }[] = [
  {
    name: "git",
    svg: (
      <span className="font-semibold tracking-tight">git</span>
    ),
  },
  {
    name: "GitHub CLI",
    svg: (
      <span className="flex items-center gap-1.5">
        <GitHubIcon width={18} height={18} />
        gh
      </span>
    ),
  },
  {
    name: "lazygit",
    svg: <span className="font-semibold tracking-tight">lazygit</span>,
  },
  {
    name: "tree-sitter",
    svg: (
      <span className="flex items-center gap-1.5">
        <TreeGlyph />
        tree-sitter
      </span>
    ),
  },
  {
    name: "Claude Code",
    svg: (
      <span className="flex items-center gap-1.5">
        <SparkGlyph />
        Claude Code
      </span>
    ),
  },
  {
    name: "pi",
    svg: (
      <span className="flex items-center gap-1.5">
        <PiGlyph />
        pi
      </span>
    ),
  },
];

export function Tooling() {
  return (
    <section className="border-b border-line/60 bg-bg-elev/30">
      <div className="mx-auto w-full max-w-6xl px-4 py-14 sm:px-6">
        <Reveal>
          <p className="text-center text-sm font-medium uppercase tracking-[0.18em] text-fg-dim">
            Plugs into the tools you already use
          </p>
        </Reveal>
        <Reveal delay={0.1}>
          <ul className="mt-8 flex flex-wrap items-center justify-center gap-x-10 gap-y-6 text-fg-muted">
            {TOOLS.map((t) => (
              <li
                key={t.name}
                className="flex items-center text-base opacity-80 transition-opacity hover:opacity-100"
              >
                {t.svg}
              </li>
            ))}
          </ul>
        </Reveal>
      </div>
    </section>
  );
}

function TreeGlyph() {
  return (
    <svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
      <circle cx="6" cy="6" r="2" />
      <circle cx="18" cy="6" r="2" />
      <circle cx="12" cy="18" r="2" />
      <path d="M6 8v3a3 3 0 0 0 3 3h6a3 3 0 0 0 3-3V8" />
      <path d="M12 14v2" />
    </svg>
  );
}

function SparkGlyph() {
  return (
    <svg viewBox="0 0 24 24" width="18" height="18" fill="currentColor" aria-hidden>
      <path d="M12 2l1.6 5.4L19 9l-5.4 1.6L12 16l-1.6-5.4L5 9l5.4-1.6L12 2z" />
      <path d="M19 14l.8 2.7L22 17.5l-2.2.8L19 21l-.8-2.7L16 17.5l2.2-.8L19 14z" />
    </svg>
  );
}

function PiGlyph() {
  return (
    <svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
      <path d="M5 7h14" />
      <path d="M9 7v10" />
      <path d="M15 7v8a2 2 0 0 0 2 2" />
    </svg>
  );
}

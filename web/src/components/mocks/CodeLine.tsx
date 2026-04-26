import * as React from "react";
import { cn } from "@/lib/utils";

export type LineKind = "plus" | "minus" | "scope" | "ctx" | "header" | "hunk";

const LINE_CLASS: Record<LineKind, string> = {
  plus: "bg-emerald-500/10 text-emerald-300",
  minus: "bg-rose-500/10 text-rose-300",
  scope: "bg-accent/[0.06] text-fg-muted",
  ctx: "text-fg-muted",
  header: "text-fg-dim",
  hunk: "bg-accent/10 text-accent",
};

const SIGIL: Record<LineKind, string> = {
  plus: "+",
  minus: "-",
  scope: " ",
  ctx: " ",
  header: " ",
  hunk: " ",
};

/**
 * One line of a hand-rendered diff. Children are the tokenized contents
 * (everything after the +/- sigil); the sigil is supplied automatically.
 */
export function CodeLine({
  kind = "ctx",
  lineNo,
  children,
  className,
}: {
  kind?: LineKind;
  lineNo?: number | string;
  children: React.ReactNode;
  className?: string;
}) {
  return (
    <div
      className={cn(
        "flex min-h-[1.55em] items-baseline whitespace-pre",
        LINE_CLASS[kind],
        className,
      )}
    >
      {lineNo !== undefined ? (
        <span className="w-10 select-none pr-2 text-right text-fg-dim/70 tabular-nums">
          {lineNo}
        </span>
      ) : null}
      <span className="w-4 select-none text-center opacity-70">
        {SIGIL[kind]}
      </span>
      <span className="flex-1">{children}</span>
    </div>
  );
}

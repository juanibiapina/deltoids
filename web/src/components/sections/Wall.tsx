import * as React from "react";
import {
  Layers,
  GitBranch,
  Gauge,
  Combine,
  Settings2,
  FileJson,
} from "lucide-react";
import { Card, CardDescription, CardTitle } from "@/components/ui/card";
import { Reveal } from "@/components/Reveal";

const ITEMS = [
  {
    icon: Layers,
    title: "Tree-sitter scope",
    body:
      "Real grammar, not regex. Knows that `}` closes the right brace and that classes contain methods.",
  },
  {
    icon: GitBranch,
    title: "Diff-op-aware",
    body:
      "Scope is resolved against both the old and new file, so renames, splits, and reorderings stay correct.",
  },
  {
    icon: Combine,
    title: "Hunk merging",
    body:
      "Adjacent hunks that share a scope collapse into one — fewer @@ headers, more readable reviews.",
  },
  {
    icon: Gauge,
    title: "Bounded budget",
    body:
      "A 200-line scope cap keeps huge functions from drowning the diff. Tunable per call site.",
  },
  {
    icon: Settings2,
    title: "Zero config",
    body:
      "No flags, no config file, no LSP, no daemon. Pipe a diff in, get a diff out.",
  },
  {
    icon: FileJson,
    title: "JSONL traces",
    body:
      "edit and write log every change as a JSONL entry under $XDG_DATA_HOME — diffable, scriptable, archivable.",
  },
];

export function Wall() {
  return (
    <section
      id="whats-inside"
      className="border-b border-line/60"
    >
      <div className="mx-auto w-full max-w-6xl px-4 py-20 sm:px-6 md:py-28">
        <Reveal>
          <p className="text-sm font-medium uppercase tracking-[0.16em] text-accent">
            What&apos;s inside
          </p>
          <h2 className="mt-3 max-w-2xl text-balance text-[clamp(1.75rem,3.4vw,2.5rem)] font-semibold leading-tight tracking-tight">
            Small surface, careful guarantees.
          </h2>
          <p className="mt-4 max-w-2xl text-pretty text-base text-fg-muted">
            deltoids is a focused tool, not a platform. Six properties make
            up the whole product surface.
          </p>
        </Reveal>
        <div className="mt-12 grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
          {ITEMS.map((it, i) => (
            <Reveal key={it.title} delay={(i % 3) * 0.05}>
              <Card className="h-full">
                <div className="flex size-9 items-center justify-center rounded-lg bg-accent/10 text-accent ring-1 ring-accent/20">
                  <it.icon className="size-4.5" />
                </div>
                <CardTitle className="mt-5">{it.title}</CardTitle>
                <CardDescription className="mt-2">{it.body}</CardDescription>
              </Card>
            </Reveal>
          ))}
        </div>
      </div>
    </section>
  );
}

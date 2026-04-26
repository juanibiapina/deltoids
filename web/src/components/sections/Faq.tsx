import * as React from "react";
import {
  Accordion,
  AccordionContent,
  AccordionItem,
  AccordionTrigger,
} from "@/components/ui/accordion";
import { Reveal } from "@/components/Reveal";

const FAQ: { q: string; a: React.ReactNode }[] = [
  {
    q: "How is this different from `git diff -W`?",
    a: (
      <>
        <code className="font-mono text-fg">git diff -W</code> finds scope by
        regex (the <code className="font-mono text-fg">xfuncname</code>{" "}
        patterns from gitattributes). deltoids parses the file with
        tree-sitter, so it handles nested classes, multi-line signatures, and
        languages where braces don&apos;t mark scope.
      </>
    ),
  },
  {
    q: "Which languages are supported?",
    a: (
      <>
        Anything with a tree-sitter grammar. The default build ships with the
        common ones (Rust, TypeScript, JavaScript, Python, Go, etc.); other
        files fall back to standard three-line context.
      </>
    ),
  },
  {
    q: "Will it slow down my pager?",
    a: (
      <>
        deltoids streams its input. For typical PR-sized diffs, the
        tree-sitter parse cost is dominated by I/O, and the scope budget caps
        worst-case expansion at ~200 lines per hunk.
      </>
    ),
  },
  {
    q: "How do I install it?",
    a: (
      <>
        From source today:{" "}
        <code className="font-mono text-fg">
          cargo install --path crates/deltoids-cli
        </code>{" "}
        plus{" "}
        <code className="font-mono text-fg">
          cargo install --path crates/edit-cli
        </code>
        . Pre-built binaries are on the roadmap.
      </>
    ),
  },
  {
    q: "Does it work with my coding agent?",
    a: (
      <>
        Yes if your agent uses <code className="font-mono text-fg">edit</code>{" "}
        and <code className="font-mono text-fg">write</code> binaries on PATH
        — that includes pi out of the box. A Claude Code integration is
        coming. For others, the JSONL trace format is the integration point.
      </>
    ),
  },
  {
    q: "Is it stable?",
    a: (
      <>
        It&apos;s in beta. The CLI surface is small and unlikely to change;
        the JSONL trace format is versioned and we treat it as a public
        contract.
      </>
    ),
  },
  {
    q: "What's the license?",
    a: <>MIT.</>,
  },
];

export function Faq() {
  return (
    <section id="faq" className="border-b border-line/60 bg-bg-elev/40">
      <div className="mx-auto w-full max-w-3xl px-4 py-20 sm:px-6 md:py-28">
        <Reveal>
          <p className="text-sm font-medium uppercase tracking-[0.16em] text-accent">
            FAQ
          </p>
          <h2 className="mt-3 text-balance text-[clamp(1.75rem,3.4vw,2.5rem)] font-semibold leading-tight tracking-tight">
            Questions, answered.
          </h2>
        </Reveal>
        <Reveal delay={0.05}>
          <Accordion
            type="single"
            collapsible
            className="mt-8 w-full"
            defaultValue="item-0"
          >
            {FAQ.map((f, i) => (
              <AccordionItem key={i} value={`item-${i}`}>
                <AccordionTrigger>{f.q}</AccordionTrigger>
                <AccordionContent>{f.a}</AccordionContent>
              </AccordionItem>
            ))}
          </Accordion>
        </Reveal>
      </div>
    </section>
  );
}

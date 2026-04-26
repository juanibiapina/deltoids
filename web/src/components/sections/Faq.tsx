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
        <code className="font-mono text-fg">git diff -W</code> finds scope
        with regex.{" "}
        <code className="font-mono text-fg">deltoids</code> parses the file
        with tree-sitter.
      </>
    ),
  },
];

export function Faq() {
  return (
    <section id="faq" className="border-b border-line/60 bg-bg-elev/40">
      <div className="mx-auto w-full max-w-3xl px-4 py-20 sm:px-6 md:py-28">
        <Reveal>
          <h2 className="text-balance text-[clamp(1.75rem,3.4vw,2.5rem)] font-semibold leading-tight tracking-tight">
            FAQ
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

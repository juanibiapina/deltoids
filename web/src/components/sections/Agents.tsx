"use client";

import * as React from "react";
import { Reveal } from "@/components/Reveal";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { CopyBlock } from "@/components/ui/CopyBlock";
import { AgentTraceMock } from "@/components/mocks/AgentTraceMock";

/**
 * Coding-agents section. Mirror of `Pager`: a short title, tabbed
 * copy-pasteable install snippets per agent, the `edit-tui` screenshot
 * underneath, and a small JSONL aside.
 */
const AGENTS: {
  id: string;
  label: string;
  language: string;
  snippet: string;
  coming?: boolean;
}[] = [
  {
    id: "pi",
    label: "pi",
    language: "bash",
    snippet: "pi install https://github.com/juanibiapina/deltoids\nedit-tui",
  },
  {
    id: "claude",
    label: "Claude Code",
    language: "bash",
    snippet: "",
    coming: true,
  },
];

export function Agents() {
  return (
    <section id="agents" className="border-b border-line/60 bg-bg-elev/40">
      <div className="mx-auto w-full max-w-6xl px-4 py-20 sm:px-6 md:py-28">
        <Reveal className="mx-auto max-w-3xl text-center">
          <h2 className="text-balance text-[clamp(1.75rem,3.4vw,2.5rem)] font-semibold leading-tight tracking-tight">
            Coding agents.
          </h2>
          <p className="mx-auto mt-5 max-w-2xl text-pretty text-base leading-relaxed text-fg-muted">
            Every agent edit, with the agent&apos;s reason and deltoids
            context.
          </p>
        </Reveal>
        <Reveal delay={0.05} className="mx-auto mt-8 max-w-2xl">
          <Tabs defaultValue="pi">
            <div className="flex justify-center">
              <TabsList>
                {AGENTS.map((a) => (
                  <TabsTrigger key={a.id} value={a.id}>
                    {a.label}
                  </TabsTrigger>
                ))}
              </TabsList>
            </div>
            {AGENTS.map((a) => (
              <TabsContent key={a.id} value={a.id}>
                {a.coming ? (
                  <div className="rounded-lg border border-line bg-bg-elev/80 px-4 py-6 text-center text-sm text-fg-muted">
                    Coming soon.
                  </div>
                ) : (
                  <CopyBlock text={a.snippet} language={a.language} />
                )}
              </TabsContent>
            ))}
          </Tabs>
        </Reveal>
        <Reveal delay={0.1} y={20} className="mt-12 md:mt-16">
          <AgentTraceMock />
        </Reveal>
        <Reveal delay={0.12} className="mx-auto mt-6 max-w-2xl text-center text-sm text-fg-dim">
          Traces are plain JSONL on disk under{" "}
          <code className="font-mono text-fg">
            $XDG_DATA_HOME/edit/traces/
          </code>
          .
        </Reveal>
      </div>
    </section>
  );
}

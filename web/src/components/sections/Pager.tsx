"use client";

import * as React from "react";
import { Reveal } from "@/components/Reveal";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { CopyBlock } from "@/components/ui/CopyBlock";
import { PagerMock } from "@/components/mocks/PagerMock";

/**
 * Diff-pager section: title, tabbed copy-pasteable setup snippets per
 * git tool, and the lazygit screenshot underneath as visual proof.
 */
const TOOLS: {
  id: string;
  label: string;
  language: string;
  snippet: string;
}[] = [
  {
    id: "git",
    label: "git",
    language: "bash",
    snippet:
      "# pipe a single diff\ngit diff | deltoids | less -R\n\n# or set as the default git pager\ngit config --global core.pager 'deltoids | less -R'",
  },
  {
    id: "gh",
    label: "gh",
    language: "bash",
    snippet: "gh pr diff <number> | deltoids | less -R",
  },
  {
    id: "lazygit",
    label: "lazygit",
    language: "yaml",
    snippet: `# ~/.config/lazygit/config.yml
git:
  paging:
    pager: deltoids`,
  },
];

export function Pager() {
  return (
    <section id="pager" className="border-b border-line/60">
      <div className="mx-auto w-full max-w-6xl px-4 py-20 sm:px-6 md:py-28">
        <Reveal className="mx-auto max-w-3xl text-center">
          <h2 className="text-balance text-[clamp(1.75rem,3.4vw,2.5rem)] font-semibold leading-tight tracking-tight">
            Diff pager.
          </h2>
        </Reveal>
        <Reveal delay={0.05} className="mx-auto mt-8 max-w-2xl">
          <Tabs defaultValue="git">
            <div className="flex justify-center">
              <TabsList>
                {TOOLS.map((t) => (
                  <TabsTrigger key={t.id} value={t.id}>
                    {t.label}
                  </TabsTrigger>
                ))}
              </TabsList>
            </div>
            {TOOLS.map((t) => (
              <TabsContent key={t.id} value={t.id}>
                <CopyBlock text={t.snippet} language={t.language} />
              </TabsContent>
            ))}
          </Tabs>
        </Reveal>
        <Reveal delay={0.1} y={20} className="mt-12 md:mt-16">
          <PagerMock />
        </Reveal>
      </div>
    </section>
  );
}

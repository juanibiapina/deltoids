import * as React from "react";
import { ArrowRight } from "lucide-react";
import { GitHubIcon } from "@/components/icons/GitHubIcon";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { EditTuiMock } from "@/components/mocks/EditTuiMock";
import { Reveal } from "@/components/Reveal";

export function Hero() {
  return (
    <section className="relative overflow-hidden border-b border-line/60">
      {/* Soft accent gradient backdrop. */}
      <div
        aria-hidden
        className="pointer-events-none absolute inset-0 -z-10 bg-[radial-gradient(60rem_30rem_at_50%_-10%,color-mix(in_oklab,var(--color-accent)_18%,transparent),transparent)]"
      />
      <div className="mx-auto grid w-full max-w-6xl gap-14 px-4 py-20 sm:px-6 md:py-28 lg:grid-cols-[minmax(0,1fr)_minmax(0,1.1fr)] lg:items-center lg:gap-16">
        <div>
          <Reveal>
            <Badge variant="accent" className="mb-5">
              <span className="mr-1.5 size-1.5 rounded-full bg-accent-strong" />
              Beta · tree-sitter scope expansion
            </Badge>
          </Reveal>
          <Reveal delay={0.05}>
            <h1 className="text-balance text-[clamp(2.25rem,5vw,3.75rem)] font-semibold leading-[1.05] tracking-tight">
              Diffs with{" "}
              <span className="bg-gradient-to-r from-accent-strong to-accent bg-clip-text text-transparent">
                context.
              </span>
            </h1>
          </Reveal>
          <Reveal delay={0.1}>
            <p className="mt-5 max-w-xl text-pretty text-lg leading-relaxed text-fg-muted">
              <span className="font-mono text-fg">deltoids</span> expands every
              hunk in a unified diff to include the entire enclosing function,
              class, or block. Scope is resolved with tree-sitter — no regex
              guesswork, no missing braces.
            </p>
          </Reveal>
          <Reveal delay={0.15}>
            <div className="mt-8 flex flex-wrap items-center gap-3">
              <Button asChild size="lg">
                <a href="#install">
                  Install
                  <ArrowRight />
                </a>
              </Button>
              <Button asChild size="lg" variant="secondary">
                <a
                  href="https://github.com/juanibiapina/deltoids"
                  target="_blank"
                  rel="noreferrer"
                >
                  <GitHubIcon />
                  Star on GitHub
                </a>
              </Button>
            </div>
          </Reveal>
          <Reveal delay={0.2}>
            <p className="mt-5 font-mono text-xs text-fg-dim">
              MIT · single binary · no config
            </p>
          </Reveal>
        </div>

        <Reveal delay={0.1} y={24}>
          <div className="relative">
            {/* Floating glow under the mock. */}
            <div
              aria-hidden
              className="pointer-events-none absolute inset-x-8 -bottom-8 h-24 rounded-[40%] bg-accent/30 blur-3xl"
            />
            <EditTuiMock />
          </div>
        </Reveal>
      </div>
    </section>
  );
}

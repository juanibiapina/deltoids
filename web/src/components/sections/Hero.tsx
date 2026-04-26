import * as React from "react";
import { ArrowRight } from "lucide-react";
import { GitHubIcon } from "@/components/icons/GitHubIcon";
import { Button } from "@/components/ui/button";
import { DeltaVsDeltoidsMock } from "@/components/mocks/DeltaVsDeltoidsMock";
import { Reveal } from "@/components/Reveal";

export function Hero() {
  return (
    <section className="relative overflow-hidden border-b border-line/60">
      {/* Soft accent gradient backdrop. */}
      <div
        aria-hidden
        className="pointer-events-none absolute inset-0 -z-10 bg-[radial-gradient(60rem_30rem_at_50%_-10%,color-mix(in_oklab,var(--color-accent)_18%,transparent),transparent)]"
      />
      <div className="mx-auto w-full max-w-6xl px-4 pt-20 pb-12 sm:px-6 md:pt-28 md:pb-16">
        <div className="mx-auto max-w-3xl text-center">
          <Reveal>
            <h1 className="text-balance text-[clamp(2.25rem,5vw,3.75rem)] font-semibold leading-[1.05] tracking-tight">
              Diffs with{" "}
              <span className="bg-gradient-to-r from-accent-strong to-accent bg-clip-text text-transparent">
                context.
              </span>
            </h1>
          </Reveal>
          <Reveal delay={0.05}>
            <p className="mx-auto mt-5 max-w-2xl text-pretty text-lg leading-relaxed text-fg-muted">
              <span className="font-mono text-fg">deltoids</span> expands
              every hunk in a unified diff to include the enclosing function,
              class, or block. Scope is resolved with tree-sitter.
            </p>
          </Reveal>
          <Reveal delay={0.1}>
            <div className="mt-8 flex flex-wrap items-center justify-center gap-3">
              <Button asChild size="lg">
                <a
                  href="https://github.com/juanibiapina/deltoids"
                  target="_blank"
                  rel="noreferrer"
                >
                  <GitHubIcon />
                  View on GitHub
                  <ArrowRight />
                </a>
              </Button>
            </div>
          </Reveal>
        </div>
      </div>

      <div className="mx-auto w-full max-w-6xl px-4 pb-20 sm:px-6 md:pb-28">
        <Reveal delay={0.1} y={24}>
          <DeltaVsDeltoidsMock />
        </Reveal>
      </div>
    </section>
  );
}

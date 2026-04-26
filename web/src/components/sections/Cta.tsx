import * as React from "react";
import { ArrowRight } from "lucide-react";
import { GitHubIcon } from "@/components/icons/GitHubIcon";
import { Button } from "@/components/ui/button";
import { Reveal } from "@/components/Reveal";
import { MacWindow } from "@/components/mocks/MacWindow";
import { Tok } from "@/components/mocks/Tok";

export function Cta() {
  return (
    <section
      id="install"
      className="relative overflow-hidden border-b border-line/60"
    >
      <div
        aria-hidden
        className="pointer-events-none absolute inset-0 -z-10 bg-[radial-gradient(48rem_24rem_at_50%_120%,color-mix(in_oklab,var(--color-accent)_22%,transparent),transparent)]"
      />
      <div className="mx-auto grid w-full max-w-5xl gap-10 px-4 py-24 sm:px-6 md:py-32 lg:grid-cols-[1fr_minmax(0,28rem)] lg:items-center">
        <Reveal>
          <p className="text-sm font-medium uppercase tracking-[0.16em] text-accent">
            Get started
          </p>
          <h2 className="mt-3 text-balance text-[clamp(2rem,4vw,3rem)] font-semibold leading-tight tracking-tight">
            Install in 30 seconds.
          </h2>
          <p className="mt-4 max-w-lg text-pretty text-base text-fg-muted">
            One Cargo command for the diff pager, one for the agent edit
            tools. Both are single binaries; both go on{" "}
            <code className="font-mono text-fg">$PATH</code>.
          </p>
          <div className="mt-7 flex flex-wrap gap-3">
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
            <Button asChild size="lg" variant="secondary">
              <a href="#features">See what it does</a>
            </Button>
          </div>
        </Reveal>

        <Reveal delay={0.08} y={20}>
          <MacWindow title="install.sh">
            <div className="text-[12.5px]">
              <Line>
                <Tok kind="comment"># Diff pager</Tok>
              </Line>
              <Line>
                <Tok kind="ident">cargo</Tok>{" "}
                <Tok kind="ident">install</Tok>{" "}
                <Tok kind="punct">--</Tok>
                <Tok kind="ident">path</Tok>{" "}
                <Tok kind="string">crates/deltoids-cli</Tok>
              </Line>
              <Line>
                <Tok kind="comment"># Agent edit tools + TUI viewer</Tok>
              </Line>
              <Line>
                <Tok kind="ident">cargo</Tok>{" "}
                <Tok kind="ident">install</Tok>{" "}
                <Tok kind="punct">--</Tok>
                <Tok kind="ident">path</Tok>{" "}
                <Tok kind="string">crates/edit-cli</Tok>
              </Line>
              <Line>
                <Tok kind="comment"># Use as your default git pager</Tok>
              </Line>
              <Line>
                <Tok kind="ident">git config </Tok>
                <Tok kind="punct">--</Tok>
                <Tok kind="ident">global core.pager </Tok>
                <Tok kind="string">&apos;deltoids | less -R&apos;</Tok>
              </Line>
            </div>
          </MacWindow>
        </Reveal>
      </div>
    </section>
  );
}

function Line({ children }: { children: React.ReactNode }) {
  return (
    <div className="flex items-baseline whitespace-pre">
      <span className="select-none pr-2 text-fg-dim/70">$</span>
      <span className="flex-1">{children}</span>
    </div>
  );
}

import * as React from "react";
import { cn } from "@/lib/utils";
import { Reveal } from "@/components/Reveal";

interface FeatureProps {
  id?: string;
  eyebrow: string;
  title: React.ReactNode;
  description: React.ReactNode;
  bullets?: React.ReactNode[];
  /** Mock side. `right` = mock on the right column (default). */
  align?: "left" | "right";
  /** Alternate background to give the page a striped rhythm. */
  alt?: boolean;
  mock: React.ReactNode;
}

export function Feature({
  id,
  eyebrow,
  title,
  description,
  bullets,
  align = "right",
  alt = false,
  mock,
}: FeatureProps) {
  return (
    <section
      id={id}
      className={cn(
        "border-b border-line/60",
        alt ? "bg-bg-elev/40" : "bg-transparent",
      )}
    >
      <div
        className={cn(
          "mx-auto grid w-full max-w-6xl gap-12 px-4 py-20 sm:px-6 md:py-28 lg:grid-cols-2 lg:items-center lg:gap-16",
        )}
      >
        <Reveal
          className={cn(align === "left" ? "lg:order-2" : "lg:order-1")}
        >
          <p className="text-sm font-medium uppercase tracking-[0.16em] text-accent">
            {eyebrow}
          </p>
          <h2 className="mt-3 text-balance text-[clamp(1.75rem,3.4vw,2.5rem)] font-semibold leading-tight tracking-tight">
            {title}
          </h2>
          <div className="mt-5 max-w-xl text-pretty text-base leading-relaxed text-fg-muted">
            {description}
          </div>
          {bullets && bullets.length > 0 ? (
            <ul className="mt-6 space-y-2.5 text-fg-muted">
              {bullets.map((b, i) => (
                <li key={i} className="flex gap-3">
                  <span
                    aria-hidden
                    className="mt-2 size-1.5 shrink-0 rounded-full bg-accent"
                  />
                  <span className="text-[0.95rem] leading-7">{b}</span>
                </li>
              ))}
            </ul>
          ) : null}
        </Reveal>
        <Reveal
          delay={0.08}
          y={20}
          className={cn(align === "left" ? "lg:order-1" : "lg:order-2")}
        >
          {mock}
        </Reveal>
      </div>
    </section>
  );
}

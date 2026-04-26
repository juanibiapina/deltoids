"use client";

import * as React from "react";
import Link from "next/link";
import { Star } from "lucide-react";
import { GitHubIcon } from "@/components/icons/GitHubIcon";
import { Button } from "@/components/ui/button";
import { formatStars } from "@/lib/github";
import { cn } from "@/lib/utils";

const NAV_LINKS = [
  { href: "#features", label: "Features" },
  { href: "#faq", label: "FAQ" },
];

export function Nav({ stars }: { stars: number | null }) {
  const [scrolled, setScrolled] = React.useState(false);

  React.useEffect(() => {
    const onScroll = () => setScrolled(window.scrollY > 8);
    onScroll();
    window.addEventListener("scroll", onScroll, { passive: true });
    return () => window.removeEventListener("scroll", onScroll);
  }, []);

  return (
    <header
      className={cn(
        "sticky top-0 z-50 w-full transition-colors",
        scrolled
          ? "border-b border-line/80 bg-bg/70 backdrop-blur-xl"
          : "border-b border-transparent",
      )}
    >
      <div className="mx-auto flex h-14 w-full max-w-6xl items-center justify-between px-4 sm:px-6">
        <Link
          href="/"
          className="flex items-center gap-2 font-mono text-sm font-semibold tracking-tight text-fg"
        >
          <DeltoidsMark />
          deltoids
        </Link>
        <nav className="hidden items-center gap-7 text-sm text-fg-muted md:flex">
          {NAV_LINKS.map((l) => (
            <a
              key={l.href}
              href={l.href}
              className="transition-colors hover:text-fg"
            >
              {l.label}
            </a>
          ))}
        </nav>
        <div className="flex items-center gap-2">
          <Button asChild variant="ghost" size="sm" className="hidden sm:inline-flex">
            <a
              href="https://github.com/juanibiapina/deltoids"
              target="_blank"
              rel="noreferrer"
              aria-label={
                stars !== null
                  ? `Star deltoids on GitHub (${stars} stars)`
                  : "View deltoids on GitHub"
              }
            >
              <GitHubIcon />
              {stars !== null ? (
                <>
                  <Star className="size-3.5 fill-current" />
                  <span className="tabular-nums">{formatStars(stars)}</span>
                </>
              ) : (
                "GitHub"
              )}
            </a>
          </Button>
        </div>
      </div>
    </header>
  );
}

function DeltoidsMark() {
  // Two stacked deltas (Δ) that mirror the "diff with context" idea.
  return (
    <svg
      viewBox="0 0 24 24"
      width="20"
      height="20"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.75"
      strokeLinecap="round"
      strokeLinejoin="round"
      className="text-accent"
      aria-hidden
    >
      <path d="M4 18 L12 4 L20 18 Z" />
      <path d="M9 18 L12 12 L15 18" />
    </svg>
  );
}

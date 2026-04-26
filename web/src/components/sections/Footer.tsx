import * as React from "react";
import { GitHubIcon } from "@/components/icons/GitHubIcon";

export function Footer() {
  const year = new Date().getFullYear();
  return (
    <footer className="bg-bg">
      <div className="mx-auto flex w-full max-w-6xl flex-col gap-4 px-4 py-10 sm:flex-row sm:items-center sm:justify-between sm:px-6">
        <div className="flex items-center gap-3 font-mono text-sm">
          <span className="text-fg">deltoids</span>
          <span className="text-fg-dim">·</span>
          <span className="text-fg-muted">diffs with context</span>
        </div>
        <div className="flex items-center gap-6 text-sm text-fg-muted">
          <a
            href="https://github.com/juanibiapina/deltoids"
            target="_blank"
            rel="noreferrer"
            className="flex items-center gap-1.5 transition-colors hover:text-fg"
          >
            <GitHubIcon className="size-4" />
            GitHub
          </a>
          <a
            href="https://github.com/juanibiapina/deltoids/blob/main/LICENSE"
            target="_blank"
            rel="noreferrer"
            className="transition-colors hover:text-fg"
          >
            MIT
          </a>
          <span className="text-fg-dim">© {year}</span>
        </div>
      </div>
    </footer>
  );
}

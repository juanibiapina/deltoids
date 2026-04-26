import * as React from "react";

export function Footer() {
  return (
    <footer className="bg-bg">
      <div className="mx-auto flex w-full max-w-6xl flex-col gap-4 px-4 py-10 sm:flex-row sm:items-center sm:justify-between sm:px-6">
        <div className="flex items-center gap-3 font-mono text-sm">
          <span className="text-fg">deltoids</span>
          <span className="text-fg-dim">·</span>
          <span className="text-fg-muted">diffs with context</span>
        </div>
        <a
          href="https://github.com/juanibiapina/deltoids/blob/main/LICENSE"
          target="_blank"
          rel="noreferrer"
          className="text-sm text-fg-muted transition-colors hover:text-fg"
        >
          MIT licensed
        </a>
      </div>
    </footer>
  );
}

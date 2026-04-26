"use client";

import * as React from "react";
import { Check, Copy } from "lucide-react";
import { cn } from "@/lib/utils";

/**
 * Copy-to-clipboard code block. Renders the given text as monospace
 * `<pre>` with a Copy button in the top-right corner; the button flips
 * to a checkmark for ~1.5s after a successful copy.
 */
export function CopyBlock({
  text,
  language,
  className,
}: {
  text: string;
  /** Optional language label shown subtly in the chrome (e.g. "bash"). */
  language?: string;
  className?: string;
}) {
  const [copied, setCopied] = React.useState(false);

  const onCopy = React.useCallback(async () => {
    try {
      await navigator.clipboard.writeText(text);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1500);
    } catch {
      // Clipboard write blocked (e.g. insecure context); fail silently.
    }
  }, [text]);

  return (
    <div
      className={cn(
        "group relative overflow-hidden rounded-lg border border-line bg-bg-elev/80 ring-1 ring-white/[0.03]",
        className,
      )}
    >
      {language ? (
        <span className="absolute left-3 top-2 select-none font-mono text-[10.5px] uppercase tracking-[0.14em] text-fg-dim">
          {language}
        </span>
      ) : null}
      <button
        type="button"
        onClick={onCopy}
        aria-label={copied ? "Copied" : "Copy to clipboard"}
        className="absolute right-2 top-2 inline-flex size-7 items-center justify-center rounded-md border border-line bg-bg-card/80 text-fg-muted opacity-0 transition-all hover:bg-bg-card hover:text-fg focus:opacity-100 group-hover:opacity-100"
      >
        {copied ? (
          <Check className="size-3.5 text-accent-strong" />
        ) : (
          <Copy className="size-3.5" />
        )}
      </button>
      <pre
        className={cn(
          "overflow-x-auto p-4 font-mono text-[12.5px] leading-[1.7] text-fg",
          language ? "pt-7" : "",
        )}
      >
        <code>{text}</code>
      </pre>
    </div>
  );
}

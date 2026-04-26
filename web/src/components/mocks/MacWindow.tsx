import * as React from "react";
import { cn } from "@/lib/utils";

interface MacWindowProps extends React.HTMLAttributes<HTMLDivElement> {
  title?: string;
  /**
   * Render the children with no internal padding. Useful when the inner
   * content is itself a multi-pane layout that handles its own gutters.
   */
  flush?: boolean;
}

/**
 * Hand-rendered macOS window chrome: rounded card with a title bar that
 * carries the three traffic-light dots and an optional centered title.
 *
 * Wraps every product mock so the page reads as a screenshot wall without
 * actually shipping any screenshots.
 */
export function MacWindow({
  title,
  flush = false,
  className,
  children,
  ...props
}: MacWindowProps) {
  return (
    <div
      className={cn(
        "relative overflow-hidden rounded-xl border border-line bg-bg-elev shadow-2xl shadow-black/40 ring-1 ring-white/[0.04]",
        className,
      )}
      {...props}
    >
      <div className="flex items-center gap-2 border-b border-line bg-bg-card px-4 py-2.5">
        <span className="size-3 rounded-full bg-[#ff5f57]" />
        <span className="size-3 rounded-full bg-[#febc2e]" />
        <span className="size-3 rounded-full bg-[#28c840]" />
        {title ? (
          <span className="ml-3 select-none truncate font-mono text-xs text-fg-dim">
            {title}
          </span>
        ) : null}
      </div>
      <div className={cn(flush ? "" : "p-4", "font-mono text-[12.5px] leading-[1.55]")}>{children}</div>
    </div>
  );
}

import * as React from "react";
import { MacWindow } from "./MacWindow";
import { CodeLine } from "./CodeLine";
import { Tok } from "./Tok";
import { cn } from "@/lib/utils";

interface Entry {
  i: number;
  tool: "edit" | "write";
  path: string;
  reason: string;
  active?: boolean;
}

const ENTRIES: Entry[] = [
  { i: 7, tool: "edit", path: "scope.rs", reason: "extract collect_insert_lines helper" },
  { i: 8, tool: "edit", path: "scope.rs", reason: "fix scope budget for trailing newline" },
  { i: 9, tool: "edit", path: "scope.rs", reason: "merge sibling pair lines into hunk", active: true },
  { i: 10, tool: "write", path: "tests/scope.rs", reason: "add test for empty range expansion" },
  { i: 11, tool: "edit", path: "parse.rs", reason: "rename inner span → range" },
];

/**
 * Vertical timeline of agent edits with the inline reason for each, and an
 * inline preview of the currently expanded entry. Distinct from
 * `EditTuiMock` (the three-pane TUI) — this is a more "story-shaped"
 * view that emphasises the per-edit reasoning.
 */
export function AgentTraceMock() {
  return (
    <MacWindow title="trace 01KQ4FXKD3 — deltoids">
      <ol className="relative space-y-3 pl-6">
        {/* Timeline rail */}
        <span
          aria-hidden
          className="absolute left-2 top-1.5 bottom-1.5 w-px bg-line"
        />
        {ENTRIES.map((e) => (
          <li key={e.i} className="relative">
            <span
              aria-hidden
              className={cn(
                "absolute -left-[18px] top-1 size-2 rounded-full border-2",
                e.active
                  ? "border-accent-strong bg-accent"
                  : "border-line bg-bg-elev",
              )}
            />
            <div
              className={cn(
                "flex items-baseline gap-2 text-[12px]",
                e.active ? "text-fg" : "text-fg-muted",
              )}
            >
              <span className="font-mono text-fg-dim">#{e.i}</span>
              <span
                className={cn(
                  "rounded-sm px-1.5 py-0.5 font-mono text-[10.5px]",
                  e.tool === "write"
                    ? "bg-emerald-500/15 text-emerald-300"
                    : "bg-accent/15 text-accent-strong",
                )}
              >
                {e.tool}
              </span>
              <span className="font-mono text-fg-dim">{e.path}</span>
            </div>
            <div className="mt-1 text-[12.5px] text-fg-muted">
              {e.reason}
            </div>
            {e.active ? (
              <div className="mt-2 overflow-hidden rounded-md border border-line bg-bg/70 [mask-image:linear-gradient(to_right,black_94%,transparent)]">
                <CodeLine kind="hunk">
                  <Tok kind="dim">@@ -147 +147 @@ fn </Tok>
                  <Tok kind="fn">collect_insert_lines</Tok>
                </CodeLine>
                <CodeLine kind="minus">
                  {"  "}
                  <Tok kind="keyword">if</Tok>{" "}
                  <Tok kind="ident">range</Tok>
                  <Tok kind="punct">.</Tok>
                  <Tok kind="fn">is_empty</Tok>
                  <Tok kind="punct">() {`{ return; }`}</Tok>
                </CodeLine>
                <CodeLine kind="plus">
                  {"  "}
                  <Tok kind="keyword">let</Tok>{" "}
                  <Tok kind="ident">budget</Tok> ={" "}
                  <Tok kind="type">MAX</Tok>
                  <Tok kind="punct">.</Tok>
                  <Tok kind="fn">min</Tok>
                  <Tok kind="punct">(</Tok>
                  <Tok kind="ident">range</Tok>
                  <Tok kind="punct">.</Tok>
                  <Tok kind="fn">len</Tok>
                  <Tok kind="punct">());</Tok>
                </CodeLine>
              </div>
            ) : null}
          </li>
        ))}
      </ol>
    </MacWindow>
  );
}

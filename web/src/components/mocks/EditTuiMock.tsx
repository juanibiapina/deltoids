import * as React from "react";
import { MacWindow } from "./MacWindow";
import { CodeLine } from "./CodeLine";
import { Tok } from "./Tok";
import { cn } from "@/lib/utils";

/**
 * Three-pane mock of `edit-tui` browsing an agent trace. Left: trace list,
 * Middle: per-edit entry list with one-line reason, Right: diff for the
 * selected entry with a deltoids-style scope breadcrumb.
 *
 * Sample data is hand-shaped from a fictional refactor of
 * `crates/deltoids/src/scope.rs` so the diff reads like real code.
 */
export function EditTuiMock({ className }: { className?: string }) {
  return (
    <MacWindow
      title="edit-tui — ~/workspace/juanibiapina/deltoids"
      flush
      className={className}
    >
      <div className="grid grid-cols-[8.5rem_12rem_minmax(0,1fr)] divide-x divide-line text-[11.5px]">
        {/* Trace list */}
        <ul className="bg-bg-elev/60 py-1">
          {[
            { id: "01KQ2EDB", label: "deltoids", active: false },
            { id: "01KQ4FXK", label: "deltoids", active: true },
            { id: "01KQ7B2R", label: "deltoids", active: false },
            { id: "01KQA9QV", label: "edit-cli", active: false },
          ].map((t) => (
            <li
              key={t.id}
              className={cn(
                "flex items-center gap-1.5 overflow-hidden px-3 py-1.5",
                t.active
                  ? "bg-accent/15 text-accent-strong"
                  : "text-fg-muted",
              )}
            >
              <span className="shrink-0 font-mono text-fg-dim/80">
                {t.id}
              </span>
              <span className="truncate">{t.label}</span>
            </li>
          ))}
        </ul>

        {/* Entry list */}
        <ul className="bg-bg-elev/30 py-1">
          {[
            { i: 7, msg: "extract collect_insert_lines helper" },
            { i: 8, msg: "fix scope budget for trailing newline" },
            { i: 9, msg: "merge sibling pair lines into hunk", active: true },
            { i: 10, msg: "add test for empty range expansion" },
            { i: 11, msg: "rename inner span → range" },
          ].map((e) => (
            <li
              key={e.i}
              className={cn(
                "flex items-center gap-1.5 overflow-hidden px-3 py-1.5",
                e.active
                  ? "bg-accent/15 text-accent-strong"
                  : "text-fg-muted",
              )}
            >
              <span className="shrink-0 font-mono text-fg-dim/80">
                #{e.i}
              </span>
              <span className="truncate">{e.msg}</span>
            </li>
          ))}
        </ul>

        {/* Diff pane */}
        <div className="min-w-0 overflow-hidden bg-bg/70 px-3 py-2 [mask-image:linear-gradient(to_right,black_92%,transparent)]">
          <div className="mb-1 font-mono text-[11px] text-fg-dim">
            crates/deltoids/src/scope.rs
          </div>
          <CodeLine kind="hunk">
            <Tok kind="dim">@@ -147,9 +147,12 @@ fn </Tok>
            <Tok kind="fn">collect_insert_lines</Tok>
          </CodeLine>
          <CodeLine kind="scope" lineNo={147}>
            <Tok kind="keyword">fn</Tok>{" "}
            <Tok kind="fn">collect_insert_lines</Tok>
            <Tok kind="punct">(</Tok>
            <Tok kind="ident">range</Tok>
            <Tok kind="punct">) {`{`}</Tok>
          </CodeLine>
          <CodeLine kind="ctx" lineNo={148}>
            {"    "}
            <Tok kind="keyword">let mut</Tok>{" "}
            <Tok kind="ident">hunks</Tok> = <Tok kind="type">Vec</Tok>
            <Tok kind="punct">::</Tok>
            <Tok kind="fn">new</Tok>
            <Tok kind="punct">();</Tok>
          </CodeLine>
          <CodeLine kind="minus" lineNo={149}>
            {"    "}
            <Tok kind="keyword">if</Tok>{" "}
            <Tok kind="ident">range</Tok>
            <Tok kind="punct">.</Tok>
            <Tok kind="fn">is_empty</Tok>
            <Tok kind="punct">() {`{`}</Tok>
          </CodeLine>
          <CodeLine kind="minus" lineNo={150}>
            {"        "}
            <Tok kind="keyword">return</Tok>{" "}
            <Tok kind="ident">hunks</Tok>
            <Tok kind="punct">;</Tok>
          </CodeLine>
          <CodeLine kind="minus" lineNo={151}>
            {"    }"}
          </CodeLine>
          <CodeLine kind="plus" lineNo={149}>
            {"    "}
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
          <CodeLine kind="plus" lineNo={150}>
            {"    "}
            <Tok kind="keyword">if</Tok>{" "}
            <Tok kind="ident">budget</Tok> =={" "}
            <Tok kind="number">0</Tok>{" "}
            <Tok kind="punct">{`{`}</Tok>
          </CodeLine>
          <CodeLine kind="plus" lineNo={151}>
            {"        "}
            <Tok kind="keyword">return</Tok>{" "}
            <Tok kind="ident">hunks</Tok>
            <Tok kind="punct">;</Tok>
          </CodeLine>
          <CodeLine kind="plus" lineNo={152}>
            {"    }"}
          </CodeLine>
          <CodeLine kind="ctx" lineNo={153}>
            {"    "}
            <Tok kind="comment">{"// Walk siblings until budget exhausted."}</Tok>
          </CodeLine>
          <CodeLine kind="ctx" lineNo={154}>
            {"    "}
            <Tok kind="ident">hunks</Tok>
          </CodeLine>
          <CodeLine kind="scope" lineNo={155}>
            {"}"}
          </CodeLine>
        </div>
      </div>
      <div className="flex items-center justify-between border-t border-line bg-bg-card/60 px-3 py-1.5 font-mono text-[10.5px] text-fg-dim">
        <span>j/k move · enter open · q quit</span>
        <span>9 / 24</span>
      </div>
    </MacWindow>
  );
}

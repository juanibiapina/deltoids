import * as React from "react";
import { MacWindow } from "./MacWindow";
import { CodeLine } from "./CodeLine";
import { Tok } from "./Tok";

/**
 * Side-by-side: a tiny `delta` diff (header + 3 lines of context) against
 * the same hunk expanded to its full enclosing function by deltoids.
 * Drives Feature 1 (Tree-sitter scope expansion).
 */
export function DeltaVsDeltoidsMock() {
  return (
    <div className="grid gap-4 sm:grid-cols-2">
      <MacWindow title="git diff | delta">
        <Pane>
          <CodeLine kind="header">
            <Tok kind="dim">diff --git a/scope.rs</Tok>
          </CodeLine>
          <CodeLine kind="hunk">
            <Tok kind="dim">@@ -150,3 +150,4 @@</Tok>
          </CodeLine>
          <CodeLine kind="ctx">
            {"  "}
            <Tok kind="keyword">let</Tok>{" "}
            <Tok kind="ident">hunks</Tok> = <Tok kind="type">Vec</Tok>
            <Tok kind="punct">::</Tok>
            <Tok kind="fn">new</Tok>
            <Tok kind="punct">();</Tok>
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
          <CodeLine kind="plus">
            {"  "}
            <Tok kind="keyword">if</Tok>{" "}
            <Tok kind="ident">budget</Tok> ==
            <Tok kind="number">0</Tok>{" "}
            <Tok kind="punct">{`{ return; }`}</Tok>
          </CodeLine>
          <CodeLine kind="ctx">
            {"  "}
            <Tok kind="comment">{"// walk siblings"}</Tok>
          </CodeLine>
        </Pane>
      </MacWindow>

      <MacWindow title="git diff | deltoids">
        <Pane>
          <CodeLine kind="header">
            <Tok kind="dim">diff --git a/scope.rs</Tok>
          </CodeLine>
          <CodeLine kind="hunk">
            <Tok kind="dim">@@ -147 +147 @@ fn </Tok>
            <Tok kind="fn">collect_insert_lines</Tok>
          </CodeLine>
          <CodeLine kind="scope">
            <Tok kind="keyword">fn</Tok>{" "}
            <Tok kind="fn">collect_insert_lines</Tok>
            <Tok kind="punct">(</Tok>
            <Tok kind="ident">range</Tok>
            <Tok kind="punct">) {`{`}</Tok>
          </CodeLine>
          <CodeLine kind="ctx">
            {"  "}
            <Tok kind="keyword">let mut</Tok>{" "}
            <Tok kind="ident">hunks</Tok> = <Tok kind="type">Vec</Tok>
            <Tok kind="punct">::</Tok>
            <Tok kind="fn">new</Tok>
            <Tok kind="punct">();</Tok>
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
          <CodeLine kind="plus">
            {"  "}
            <Tok kind="keyword">if</Tok>{" "}
            <Tok kind="ident">budget</Tok> ==
            <Tok kind="number">0</Tok>{" "}
            <Tok kind="punct">{`{ return; }`}</Tok>
          </CodeLine>
          <CodeLine kind="ctx">
            {"  "}
            <Tok kind="comment">{"// walk siblings"}</Tok>
          </CodeLine>
          <CodeLine kind="ctx">
            {"  "}
            <Tok kind="ident">hunks</Tok>
          </CodeLine>
          <CodeLine kind="scope">{"}"}</CodeLine>
        </Pane>
      </MacWindow>
    </div>
  );
}

/**
 * Standard inner pane: clip overflow, fade the right edge so any line that
 * runs long looks intentional rather than truncated.
 */
function Pane({ children }: { children: React.ReactNode }) {
  return (
    <div className="min-w-0 overflow-hidden text-[11.5px] [mask-image:linear-gradient(to_right,black_94%,transparent)]">
      {children}
    </div>
  );
}

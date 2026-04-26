import * as React from "react";
import { MacWindow } from "./MacWindow";
import { CodeLine } from "./CodeLine";
import { Tok } from "./Tok";

/**
 * Terminal-style window: a shell prompt running `git diff | deltoids | less -R`
 * with the rendered output below. Drives Feature 2 (drop-in pager).
 */
export function PagerMock() {
  return (
    <MacWindow title="zsh — ~/workspace/deltoids">
      <div className="min-w-0 overflow-hidden text-[12px] [mask-image:linear-gradient(to_right,black_94%,transparent)]">
        <div className="mb-1 flex items-baseline">
          <Tok kind="prop">juan@laptop</Tok>
          <Tok kind="punct">:</Tok>
          <Tok kind="fn">~/workspace/deltoids</Tok>
          <Tok kind="punct"> $ </Tok>
          <Tok kind="ident">git diff</Tok>{" "}
          <Tok kind="punct">|</Tok>{" "}
          <Tok kind="fn">deltoids</Tok>{" "}
          <Tok kind="punct">|</Tok>{" "}
          <Tok kind="ident">less -R</Tok>
        </div>
        <CodeLine kind="header">
          <Tok kind="dim">diff --git a/parse.rs b/parse.rs</Tok>
        </CodeLine>
        <CodeLine kind="hunk">
          <Tok kind="dim">@@ -42,8 +42,11 @@ impl </Tok>
          <Tok kind="type">Diff</Tok>
        </CodeLine>
        <CodeLine kind="scope">
          {"    "}
          <Tok kind="keyword">pub fn</Tok>{" "}
          <Tok kind="fn">parse</Tok>
          <Tok kind="punct">(</Tok>
          <Tok kind="ident">input</Tok>
          <Tok kind="punct">: &amp;</Tok>
          <Tok kind="type">str</Tok>
          <Tok kind="punct">) -&gt; </Tok>
          <Tok kind="type">Result</Tok>
          <Tok kind="punct">&lt;</Tok>
          <Tok kind="type">Self</Tok>
          <Tok kind="punct">&gt; {`{`}</Tok>
        </CodeLine>
        <CodeLine kind="ctx">
          {"        "}
          <Tok kind="keyword">let</Tok>{" "}
          <Tok kind="ident">lines</Tok> ={" "}
          <Tok kind="ident">input</Tok>
          <Tok kind="punct">.</Tok>
          <Tok kind="fn">lines</Tok>
          <Tok kind="punct">();</Tok>
        </CodeLine>
        <CodeLine kind="minus">
          {"        "}
          <Tok kind="keyword">let mut</Tok>{" "}
          <Tok kind="ident">hunks</Tok> = vec<Tok kind="punct">!</Tok>
          <Tok kind="punct">[];</Tok>
        </CodeLine>
        <CodeLine kind="plus">
          {"        "}
          <Tok kind="keyword">let mut</Tok>{" "}
          <Tok kind="ident">hunks</Tok> ={" "}
          <Tok kind="type">Vec</Tok>
          <Tok kind="punct">::</Tok>
          <Tok kind="fn">with_capacity</Tok>
          <Tok kind="punct">(</Tok>
          <Tok kind="number">8</Tok>
          <Tok kind="punct">);</Tok>
        </CodeLine>
        <CodeLine kind="ctx">
          {"        "}
          <Tok kind="keyword">for</Tok>{" "}
          <Tok kind="ident">line</Tok>{" "}
          <Tok kind="keyword">in</Tok>{" "}
          <Tok kind="ident">lines</Tok>{" "}
          <Tok kind="punct">{`{`}</Tok>
        </CodeLine>
        <CodeLine kind="ctx">
          {"            "}
          <Tok kind="comment">{"// dispatch on first byte"}</Tok>
        </CodeLine>
        <CodeLine kind="ctx">{"        }"}</CodeLine>
        <CodeLine kind="ctx">
          {"        "}
          <Tok kind="type">Ok</Tok>
          <Tok kind="punct">(</Tok>
          <Tok kind="type">Self</Tok>{" "}
          <Tok kind="punct">{`{ hunks }`}</Tok>
          <Tok kind="punct">)</Tok>
        </CodeLine>
        <CodeLine kind="scope">{"    }"}</CodeLine>
      </div>
    </MacWindow>
  );
}

import * as React from "react";
import { Feature } from "./Feature";
import { DeltaVsDeltoidsMock } from "@/components/mocks/DeltaVsDeltoidsMock";
import { PagerMock } from "@/components/mocks/PagerMock";
import { AgentTraceMock } from "@/components/mocks/AgentTraceMock";
import { JsonlMock } from "@/components/mocks/JsonlMock";

export function Features() {
  return (
    <div id="features">
      <Feature
        id="scope"
        eyebrow="Tree-sitter scope expansion"
        title={<>See the whole function, not three lines around it.</>}
        description={
          <>
            Default <code className="font-mono text-fg">git diff</code> shows
            three lines of context. <code className="font-mono text-fg">deltoids</code>{" "}
            parses both files with tree-sitter and expands every hunk to the
            enclosing function, class, or block — so the next reviewer knows
            exactly what changed and what it sits inside.
          </>
        }
        bullets={[
          <>Powered by tree-sitter, not regex heuristics.</>,
          <>Configurable scope budget keeps huge functions from blowing up.</>,
          <>Falls back gracefully on languages without a grammar.</>,
        ]}
        align="right"
        mock={<DeltaVsDeltoidsMock />}
      />

      <Feature
        eyebrow="Drop-in pager"
        title={<>Reads stdin, writes stdout. Plugs in anywhere.</>}
        description={
          <>
            Pipe <code className="font-mono text-fg">git diff</code>,{" "}
            <code className="font-mono text-fg">gh pr diff</code>, or set it as
            the pager for git or lazygit. No daemon, no config file, no LSP —
            one binary that takes a unified diff in and writes one back.
          </>
        }
        bullets={[
          <>
            <code className="font-mono text-fg">git config --global core.pager
            &apos;deltoids | less -R&apos;</code>
          </>,
          <>Works as a lazygit pager out of the box.</>,
          <>Streams: starts rendering before the input is fully read.</>,
        ]}
        align="left"
        alt
        mock={<PagerMock />}
      />

      <Feature
        eyebrow="Coding-agent traces"
        title={<>Watch your agent edit, change by change.</>}
        description={
          <>
            Drop-in replacements for the standard{" "}
            <code className="font-mono text-fg">edit</code> and{" "}
            <code className="font-mono text-fg">write</code>{" "}
            tools record every change with the agent&apos;s reason. Open{" "}
            <code className="font-mono text-fg">edit-tui</code> in the same
            directory and review them with full deltoids context.
          </>
        }
        bullets={[
          <>Three-pane lazygit-style browser: traces, entries, diff.</>,
          <>One-line reason per edit — no spelunking through chat logs.</>,
          <>
            Works with pi today; Claude Code and others coming via the same
            binary contract.
          </>,
        ]}
        align="right"
        mock={<AgentTraceMock />}
      />

      <Feature
        eyebrow="Plain JSONL on disk"
        title={<>No proprietary store. Just files you can grep.</>}
        description={
          <>
            Traces live under{" "}
            <code className="font-mono text-fg">$XDG_DATA_HOME/edit/traces/</code>
            {" "}as one JSONL line per edit. Diff them, archive them, replay
            them, ship them through your existing pipelines. The format is the
            integration.
          </>
        }
        bullets={[
          <>Append-only — safe to write from concurrent agent processes.</>,
          <>One trace per task; one entry per tool call.</>,
          <>
            Drives <code className="font-mono text-fg">edit-tui</code> directly
            — the TUI is just a viewer.
          </>,
        ]}
        align="left"
        alt
        mock={<JsonlMock />}
      />
    </div>
  );
}

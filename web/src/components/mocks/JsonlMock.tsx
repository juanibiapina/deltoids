import * as React from "react";
import { MacWindow } from "./MacWindow";
import { CodeLine } from "./CodeLine";
import { Tok } from "./Tok";
import { FileText, Folder, FolderOpen } from "lucide-react";

/**
 * File tree + JSONL pane mock for Feature 4: traces are plain JSONL on
 * disk under $XDG_DATA_HOME/edit/traces/.
 */
export function JsonlMock() {
  return (
    <MacWindow title="$XDG_DATA_HOME/edit/traces" flush>
      <div className="grid grid-cols-[16rem_1fr] divide-x divide-line">
        {/* File tree */}
        <ul className="space-y-0.5 bg-bg-elev/50 py-2 text-[12px]">
          <TreeRow icon="folder-open" label="traces" depth={0} />
          <TreeRow icon="folder" label="01KQ2EDB…" depth={1} />
          <TreeRow icon="folder-open" label="01KQ4FXK…" depth={1} active />
          <TreeRow icon="file" label="entries.jsonl" depth={2} active />
          <TreeRow icon="file" label="meta.json" depth={2} />
          <TreeRow icon="folder" label="01KQ7B2R…" depth={1} />
          <TreeRow icon="folder" label="01KQA9QV…" depth={1} />
        </ul>

        {/* JSONL pane */}
        <div className="min-w-0 overflow-hidden bg-bg/70 px-3 py-2 text-[11.5px] [mask-image:linear-gradient(to_right,black_92%,transparent)]">
          <CodeLine kind="ctx" lineNo={1}>
            <JsonObj>
              <JsonField k="ts" value="2025-04-25T18:42:11Z" />
              <JsonField k="tool" value="edit" />
              <JsonField k="path" value="crates/deltoids/src/scope.rs" />
              <JsonField k="reason" value="extract collect_insert_lines" />
            </JsonObj>
          </CodeLine>
          <CodeLine kind="ctx" lineNo={2}>
            <JsonObj>
              <JsonField k="ts" value="2025-04-25T18:43:02Z" />
              <JsonField k="tool" value="edit" />
              <JsonField k="path" value="crates/deltoids/src/scope.rs" />
              <JsonField k="reason" value="fix scope budget" />
            </JsonObj>
          </CodeLine>
          <CodeLine kind="ctx" lineNo={3}>
            <JsonObj>
              <JsonField k="ts" value="2025-04-25T18:44:18Z" />
              <JsonField k="tool" value="write" />
              <JsonField k="path" value="crates/deltoids/tests/scope.rs" />
              <JsonField
                k="reason"
                value="add test for empty range expansion"
              />
            </JsonObj>
          </CodeLine>
          <CodeLine kind="ctx" lineNo={4}>
            <JsonObj>
              <JsonField k="ts" value="2025-04-25T18:46:04Z" />
              <JsonField k="tool" value="edit" />
              <JsonField k="path" value="crates/deltoids/src/parse.rs" />
              <JsonField k="reason" value="rename inner span → range" />
            </JsonObj>
          </CodeLine>
        </div>
      </div>
    </MacWindow>
  );
}

function TreeRow({
  icon,
  label,
  depth,
  active,
}: {
  icon: "folder" | "folder-open" | "file";
  label: string;
  depth: number;
  active?: boolean;
}) {
  const Icon =
    icon === "folder"
      ? Folder
      : icon === "folder-open"
        ? FolderOpen
        : FileText;
  return (
    <li
      className={[
        "flex items-center gap-1.5 px-3 py-0.5 font-mono",
        active ? "bg-accent/15 text-accent-strong" : "text-fg-muted",
      ].join(" ")}
      style={{ paddingLeft: `${0.75 + depth * 0.85}rem` }}
    >
      <Icon className="size-3.5 text-fg-dim" />
      <span className="truncate">{label}</span>
    </li>
  );
}

function JsonObj({ children }: { children: React.ReactNode }) {
  // Render as one line with a trailing space-comma layout so it stays
  // visually compact in the mock.
  const items = React.Children.toArray(children);
  return (
    <span>
      <Tok kind="punct">{"{"}</Tok>
      {items.map((child, i) => (
        <React.Fragment key={i}>
          {child}
          {i < items.length - 1 ? <Tok kind="punct">, </Tok> : null}
        </React.Fragment>
      ))}
      <Tok kind="punct">{"}"}</Tok>
    </span>
  );
}

function JsonField({ k, value }: { k: string; value: string }) {
  return (
    <span>
      <Tok kind="prop">&quot;{k}&quot;</Tok>
      <Tok kind="punct">: </Tok>
      <Tok kind="string">&quot;{value}&quot;</Tok>
    </span>
  );
}

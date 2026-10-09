import { useEffect, useMemo, useRef } from "react";
import TreeView, { type INode } from "react-accessible-treeview";
import {
  buildTree,
  directoryIds,
  pruneReviewed,
  type Selection,
  type TreeMeta,
} from "../core/filetree";
import { fileIcon } from "./fileIcons";
import { useMediaQuery } from "../hooks/useMediaQuery";
import type { PrFile } from "../core/github";

interface FileTreeProps {
  files: PrFile[];
  onSelect: (selection: Selection) => void;
  isReviewed?: (index: number) => boolean;
  // When set, reviewed files are dropped from the tree (mirrors the global
  // "Hide viewed" toggle) instead of shown dimmed.
  hideReviewed?: boolean;
  selection?: Selection;
  // Reports each directory the tree folds or opens, including the initial
  // all-open state and the reset after a remount.
  onExpandChange?: (id: string, expanded: boolean) => void;
}

// A per-file-type brand logo, or the generic file glyph when unmapped.
function TypeIcon({ name }: { name: string }) {
  const icon = fileIcon(name);
  if (!icon) {
    return (
      <span className="tree-ico tree-ico-file">
        <FileIcon />
      </span>
    );
  }
  return (
    <span className="tree-ico" style={{ color: icon.color }}>
      <svg viewBox="0 0 24 24" width="15" height="15" fill="currentColor" aria-hidden="true">
        <path d={icon.path} />
      </svg>
    </span>
  );
}

// Single-letter change badge (A/M/D/R) in the file row's chevron column.
function statusBadge(status: string | undefined) {
  const map: Record<string, [string, string]> = {
    added: ["A", "added"],
    removed: ["D", "removed"],
    renamed: ["R", "renamed"],
    modified: ["M", "modified"],
  };
  const entry = map[status ?? "modified"] ?? map.modified;
  return <span className={`tree-twist tree-badge ${entry[1]}`}>{entry[0]}</span>;
}

// Added and removed line counts after the filename; zero counts are left out.
function lineCounts(file: PrFile | undefined) {
  const added = file?.additions ?? 0;
  const removed = file?.deletions ?? 0;
  return (
    <>
      {added > 0 && <span className="tree-delta added">+{added}</span>}
      {removed > 0 && <span className="tree-delta removed">-{removed}</span>}
    </>
  );
}

function ChevronIcon({ open }: { open: boolean }) {
  return (
    <svg
      className={`tree-chev${open ? " open" : ""}`}
      viewBox="0 0 16 16"
      width="12"
      height="12"
      fill="currentColor"
      aria-hidden="true"
    >
      <path d="M6.22 3.22a.75.75 0 0 1 1.06 0l4.25 4.25a.75.75 0 0 1 0 1.06l-4.25 4.25a.75.75 0 1 1-1.06-1.06L9.94 8 6.22 4.28a.75.75 0 0 1 0-1.06Z" />
    </svg>
  );
}

function FolderIcon() {
  return (
    <svg viewBox="0 0 16 16" width="16" height="16" fill="currentColor" aria-hidden="true">
      <path d="M1.75 1A1.75 1.75 0 0 0 0 2.75v10.5C0 14.216.784 15 1.75 15h12.5A1.75 1.75 0 0 0 16 13.25v-8.5A1.75 1.75 0 0 0 14.25 3H7.5a.25.25 0 0 1-.2-.1l-.9-1.2C6.07 1.26 5.55 1 5 1H1.75Z" />
    </svg>
  );
}

function FileIcon() {
  return (
    <svg viewBox="0 0 16 16" width="16" height="16" fill="currentColor" aria-hidden="true">
      <path d="M2 1.75C2 .784 2.784 0 3.75 0h6.586c.464 0 .909.184 1.237.513l2.914 2.914c.329.328.513.773.513 1.237v9.586A1.75 1.75 0 0 1 13.25 16h-9.5A1.75 1.75 0 0 1 2 14.25Zm1.75-.25a.25.25 0 0 0-.25.25v12.5c0 .138.112.25.25.25h9.5a.25.25 0 0 0 .25-.25V6h-2.75A1.75 1.75 0 0 1 9 4.25V1.5Zm6.75.062V4.25c0 .138.112.25.25.25h2.688l-.011-.013-2.914-2.914-.013-.011Z" />
    </svg>
  );
}

// Indent guide rails, one per ancestor level (like GitHub's file tree).
function guides(level: number) {
  return Array.from({ length: Math.max(0, level - 1) }, (_, i) => (
    <span key={i} className="tree-guide" aria-hidden="true" />
  ));
}

// Grouped, collapsible file tree. A row click selects the file or directory
// the diff column shows; a directory's chevron folds it. Fully expanded by
// default so every changed file is visible at once.
export function FileTree({
  files,
  onSelect,
  isReviewed,
  hideReviewed,
  selection,
  onExpandChange,
}: FileTreeProps) {
  const fullData = useMemo(
    () => buildTree(files.map((f) => ({ filename: f.filename, status: f.status }))),
    [files],
  );
  const data = useMemo(
    () =>
      hideReviewed && isReviewed
        ? pruneReviewed(fullData, isReviewed)
        : fullData,
    [fullData, hideReviewed, isReviewed],
  );
  const expandedIds = useMemo(() => directoryIds(data), [data]);
  // react-accessible-treeview keeps internal selection/focus state keyed by
  // node id. If the selected node is pruned from `data` (e.g. it was marked
  // viewed while "Hide viewed" is on) the library dereferences the missing id
  // and throws, unmounting the app. Remount the tree whenever the visible node
  // set changes so no stale selection can outlive a prune. The key is stable
  // across ordinary re-renders (data is memoized), so collapse state persists
  // except when a file actually enters or leaves the tree.
  const treeKey = useMemo(() => data.map((n) => n.id).join("|"), [data]);

  // Keep the selected row visible without moving the page. On wide screens
  // the sidebar is its own sticky scroll container, so nudge *its* scrollTop
  // (never `scrollIntoView`, which would also scroll the window). On the narrow
  // drawer the sidebar is off-canvas, so skip it entirely.
  const navRef = useRef<HTMLElement>(null);
  const wide = useMediaQuery("(min-width: 1024px)");
  useEffect(() => {
    if (!wide || !selection) return;
    const nav = navRef.current;
    if (!nav) return;
    const row = nav.querySelector<HTMLElement>(".tree-row.active");
    if (!row) return;
    const navRect = nav.getBoundingClientRect();
    const rowRect = row.getBoundingClientRect();
    const margin = 8;
    if (rowRect.top < navRect.top + margin) {
      nav.scrollTop -= navRect.top + margin - rowRect.top;
    } else if (rowRect.bottom > navRect.bottom - margin) {
      nav.scrollTop += rowRect.bottom - (navRect.bottom - margin);
    }
  }, [wide, selection]);

  return (
    <nav className="sidebar" ref={navRef}>
      <TreeView
        key={treeKey}
        data={data as unknown as INode[]}
        className="filetree"
        aria-label="Changed files"
        defaultExpandedIds={expandedIds}
        onExpand={({ element, isExpanded }) =>
          onExpandChange?.(String(element.id), isExpanded)
        }
        onSelect={({ element, isBranch, isSelected }) => {
          // onSelect also fires for the node being deselected; only act on the
          // newly-selected one, else clicking B scrolls to the old A.
          if (isBranch || !isSelected) return;
          const meta = element.metadata as TreeMeta | undefined;
          const index = meta?.fileIndex;
          if (index === undefined) return;
          onSelect({ kind: "file", index });
        }}
        nodeRenderer={({ element, getNodeProps, level, isBranch, isExpanded, handleExpand }) => {
          const meta = element.metadata as TreeMeta | undefined;

          if (isBranch) {
            const id = String(element.id);
            const active = selection?.kind === "dir" && selection.id === id;
            return (
              <div
                {...getNodeProps({ onClick: () => onSelect({ kind: "dir", id }) })}
                className={`tree-row tree-dir${active ? " active" : ""}`}
                aria-current={active ? "true" : undefined}
              >
                {guides(level)}
                <span
                  className="tree-twist"
                  title={isExpanded ? "Collapse" : "Expand"}
                  onClick={(event) => {
                    event.stopPropagation();
                    handleExpand(event);
                  }}
                >
                  <ChevronIcon open={isExpanded} />
                </span>
                <span className="tree-ico tree-ico-dir">
                  <FolderIcon />
                </span>
                <span className="tree-name">{element.name}</span>
              </div>
            );
          }

          const reviewed =
            meta?.fileIndex !== undefined &&
            (isReviewed?.(meta.fileIndex) ?? false);
          const active =
            selection?.kind === "file" &&
            meta?.fileIndex !== undefined &&
            meta.fileIndex === selection.index;

          return (
            <div
              {...getNodeProps()}
              className={`tree-row tree-file${reviewed ? " reviewed" : ""}${active ? " active" : ""}`}
              aria-current={active ? "true" : undefined}
              title={meta?.path ?? element.name}
            >
              {guides(level)}
              {reviewed ? (
                <span className="tree-twist tree-badge reviewed" title="Reviewed">
                  ✓
                </span>
              ) : (
                statusBadge(meta?.status)
              )}
              <TypeIcon name={element.name} />
              <span className="tree-name">{element.name}</span>
              {lineCounts(meta?.fileIndex !== undefined ? files[meta.fileIndex] : undefined)}
            </div>
          );
        }}
      />
    </nav>
  );
}

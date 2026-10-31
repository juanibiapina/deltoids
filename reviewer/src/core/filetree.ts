// The PR file list as the flat parent/children node array that
// `react-accessible-treeview` consumes. The grouping, order, and folding come
// from the shared core (`crates/deltoids-review/src/tree.rs`), the same code
// the TUI's sidebar uses; this module adds the web's selection helpers.

import { treeRows } from "./reviewCore";

export interface TreeFile {
  filename: string;
  status: string;
}

export interface TreeMeta {
  isDir: boolean;
  /** Index into the original file list (leaves only). */
  fileIndex?: number;
  /** GitHub file status (leaves only). */
  status?: string;
  /** Full path (leaves only) for the row's title tooltip. */
  path?: string;
}

/** A node in `react-accessible-treeview`'s flat `data` shape. */
export interface TreeNode {
  id: string;
  name: string;
  parent: string | null;
  children: string[];
  metadata: TreeMeta;
}

export const ROOT_ID = "__root__";

// Turn the tree rows of the shared core into nodes. A directory's id is its
// full folded path ("crates/deltoids/src"); a file's id is its path.
export function buildTree(files: TreeFile[]): TreeNode[] {
  const root: TreeNode = {
    id: ROOT_ID,
    name: "",
    parent: null,
    children: [],
    metadata: { isDir: true },
  };
  const out: TreeNode[] = [root];
  const dirs: TreeNode[] = [];
  for (const row of treeRows(files.map((f) => f.filename))) {
    const parent = row.depth === 0 ? root : dirs[row.depth - 1];
    const prefix = parent === root ? "" : `${parent.id}/`;
    let node: TreeNode;
    if (row.kind === "dir") {
      const name = row.label.slice(0, -1);
      node = { id: prefix + name, name, parent: parent.id, children: [], metadata: { isDir: true } };
      dirs.length = row.depth;
      dirs.push(node);
    } else {
      const path = prefix + row.name;
      node = {
        id: path,
        name: row.name,
        parent: parent.id,
        children: [],
        metadata: { isDir: false, fileIndex: row.index, status: files[row.index].status, path },
      };
    }
    parent.children.push(node.id);
    out.push(node);
  }
  return out;
}

/** Ids of every directory node (branches), excluding the hidden root. */
export function directoryIds(nodes: TreeNode[]): string[] {
  return nodes
    .filter((n) => n.metadata.isDir && n.id !== ROOT_ID)
    .map((n) => n.id);
}

export type Selection =
  | { kind: "file"; index: number }
  | { kind: "dir"; id: string };

function fileIndices(nodes: TreeNode[], keep: (node: TreeNode) => boolean): number[] {
  return nodes.flatMap((n) =>
    !n.metadata.isDir && n.metadata.fileIndex !== undefined && keep(n)
      ? [n.metadata.fileIndex]
      : [],
  );
}

export function displayOrder(nodes: TreeNode[]): number[] {
  return fileIndices(nodes, () => true);
}

export function selectionFiles(nodes: TreeNode[], selection: Selection): number[] {
  if (selection.kind === "file") return [selection.index];
  const prefix = `${selection.id}/`;
  return fileIndices(nodes, (n) => n.id.startsWith(prefix));
}

export interface StepOptions {
  collapsed: ReadonlySet<string>;
  isHidden?: (fileIndex: number) => boolean;
}

function asSelection(node: TreeNode): Selection {
  return node.metadata.isDir
    ? { kind: "dir", id: node.id }
    : { kind: "file", index: node.metadata.fileIndex! };
}

function selectionId(nodes: TreeNode[], selection: Selection): string | undefined {
  if (selection.kind === "dir") return selection.id;
  return nodes.find((n) => !n.metadata.isDir && n.metadata.fileIndex === selection.index)?.id;
}

// The row after (direction 1) or before (-1) the selection among `rows`. The
// selection need not be one of the rows (a hidden file, a directory when
// stepping files); it is placed by its position in the full tree. At either
// end the selection is returned unchanged.
function stepAmong(
  nodes: TreeNode[],
  selection: Selection,
  direction: 1 | -1,
  rows: TreeNode[],
): Selection {
  const position = new Map(nodes.map((n, i) => [n.id, i]));
  const current = selectionId(nodes, selection);
  if (current === undefined) return selection;
  const from = position.get(current) ?? -1;

  const next =
    direction === 1
      ? rows.find((n) => position.get(n.id)! > from)
      : [...rows].reverse().find((n) => position.get(n.id)! < from);
  return next ? asSelection(next) : selection;
}

// The sidebar row after (direction 1) or before (-1) the selection, among the
// rows the sidebar shows: hidden files and their emptied directories are
// pruned, and children of collapsed directories are skipped.
export function stepSelection(
  nodes: TreeNode[],
  selection: Selection,
  direction: 1 | -1,
  { collapsed, isHidden }: StepOptions,
): Selection {
  const shown = isHidden ? pruneReviewed(nodes, isHidden) : nodes;
  const open = new Set<string>([ROOT_ID]);
  const rows: TreeNode[] = [];
  for (const n of shown) {
    if (n.id === ROOT_ID || n.parent === null || !open.has(n.parent)) continue;
    rows.push(n);
    if (n.metadata.isDir && !collapsed.has(n.id)) open.add(n.id);
  }
  return stepAmong(nodes, selection, direction, rows);
}

// The file after (direction 1) or before (-1) the selection in tree order,
// skipping hidden files. Directories and folding do not apply: the small-screen
// swipe moves between files.
export function stepFile(
  nodes: TreeNode[],
  selection: Selection,
  direction: 1 | -1,
  isHidden?: (fileIndex: number) => boolean,
): Selection {
  const rows = nodes.filter(
    (n) =>
      !n.metadata.isDir &&
      n.metadata.fileIndex !== undefined &&
      !isHidden?.(n.metadata.fileIndex),
  );
  return stepAmong(nodes, selection, direction, rows);
}

// Drop reviewed file leaves (and any directory that becomes empty as a result)
// from an already-built tree. Used to keep the sidebar aligned with the global
// "Hide viewed" toggle: a hidden file card should not leave a dead row behind.
// Returns the original array when nothing is reviewed, so the common case does
// no work.
export function pruneReviewed(
  nodes: TreeNode[],
  isReviewed: (fileIndex: number) => boolean,
): TreeNode[] {
  const drop = new Set<string>();
  for (const n of nodes) {
    if (
      !n.metadata.isDir &&
      n.metadata.fileIndex !== undefined &&
      isReviewed(n.metadata.fileIndex)
    ) {
      drop.add(n.id);
    }
  }
  if (drop.size === 0) return nodes;

  // Clone each node with a children list that omits dropped ids.
  const byId = new Map(
    nodes.map((n) => [n.id, { ...n, children: n.children.filter((c) => !drop.has(c)) }]),
  );

  // Cascade: a non-root directory with no surviving children is itself dropped,
  // which may in turn empty its parent — repeat until stable.
  let changed = true;
  while (changed) {
    changed = false;
    for (const n of byId.values()) {
      if (drop.has(n.id)) continue;
      if (n.metadata.isDir && n.id !== ROOT_ID && n.children.length === 0) {
        drop.add(n.id);
        changed = true;
      }
    }
    if (changed) {
      for (const n of byId.values()) {
        n.children = n.children.filter((c) => !drop.has(c));
      }
    }
  }

  return nodes.filter((n) => !drop.has(n.id)).map((n) => byId.get(n.id)!);
}

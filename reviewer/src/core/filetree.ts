// Turn the flat PR file list into the flat parent/children node array that
// `react-accessible-treeview` consumes. This mirrors the CLI's grouping in
// `crates/deltoids-cli/src/sidebar/tree.rs::build_rows`:
//
//   1. Split each file's path on `/`; insert the leaf under the deepest dir.
//   2. Sort each directory's children by name, dirs and files interleaved
//      (ordinal compare, matching Rust's `str::cmp`, not locale collation).
//   3. Collapse single-child directory chains into one branch node whose name
//      is the folded path (e.g. `crates/deltoids/src`).
//
// Keeping this framework-neutral means it can be unit-tested against the
// `tree.rs` cases without importing the tree library.

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

interface Internal {
  name: string;
  fileIndex?: number;
  status?: string;
  children: Internal[];
}

function isDir(node: Internal): boolean {
  return node.fileIndex === undefined;
}

function insertPath(
  root: Internal,
  filename: string,
  fileIndex: number,
  status: string,
): void {
  const segments = filename.split("/").filter((s) => s.length > 0);
  const leaf = segments.pop();
  if (leaf === undefined) return;

  let current = root;
  for (const segment of segments) {
    let child = current.children.find((c) => isDir(c) && c.name === segment);
    if (!child) {
      child = { name: segment, children: [] };
      current.children.push(child);
    }
    current = child;
  }
  current.children.push({ name: leaf, fileIndex, status, children: [] });
}

// Ordinal name compare, matching Rust's `str::cmp` byte order for the ASCII
// paths GitHub returns (avoids locale-dependent `localeCompare`).
function byName(a: Internal, b: Internal): number {
  return a.name < b.name ? -1 : a.name > b.name ? 1 : 0;
}

function sortTree(node: Internal): void {
  node.children.sort(byName);
  for (const child of node.children) sortTree(child);
}

// Emit `node` (and its subtree) into `out`, collapsing single-child directory
// chains. Returns the emitted node's id so the parent can link it.
function emit(
  node: Internal,
  parentId: string,
  parentPath: string,
  out: TreeNode[],
): string {
  const realPath = parentPath ? `${parentPath}/${node.name}` : node.name;

  if (!isDir(node)) {
    out.push({
      id: realPath,
      name: node.name,
      parent: parentId,
      children: [],
      metadata: {
        isDir: false,
        fileIndex: node.fileIndex,
        status: node.status,
        path: realPath,
      },
    });
    return realPath;
  }

  // Fold a chain of single-child directories into one label + id.
  let label = node.name;
  let id = realPath;
  let current = node;
  while (current.children.length === 1 && isDir(current.children[0])) {
    current = current.children[0];
    label += `/${current.name}`;
    id += `/${current.name}`;
  }

  const entry: TreeNode = {
    id,
    name: label,
    parent: parentId,
    children: [],
    metadata: { isDir: true },
  };
  out.push(entry);
  for (const child of current.children) {
    entry.children.push(emit(child, id, id, out));
  }
  return id;
}

export function buildTree(files: TreeFile[]): TreeNode[] {
  const root: Internal = { name: "", children: [] };
  files.forEach((file, index) =>
    insertPath(root, file.filename, index, file.status),
  );
  sortTree(root);

  const out: TreeNode[] = [
    { id: ROOT_ID, name: "", parent: null, children: [], metadata: { isDir: true } },
  ];
  for (const child of root.children) {
    out[0].children.push(emit(child, ROOT_ID, "", out));
  }
  return out;
}

/** Ids of every directory node (branches), excluding the hidden root. */
export function directoryIds(nodes: TreeNode[]): string[] {
  return nodes
    .filter((n) => n.metadata.isDir && n.id !== ROOT_ID)
    .map((n) => n.id);
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

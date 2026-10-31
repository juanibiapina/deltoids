// The review logic shared with the TUI (crates/deltoids-review), loaded as a
// small wasm module on the main thread. `loadReviewCore` must finish before
// `treeRows` or `changeCells` runs; `openReview` in App awaits it.

export interface DirRow {
  kind: "dir";
  depth: number;
  // Folded path segments with a trailing "/" ("crates/deltoids/src/").
  label: string;
}

export interface FileRow {
  kind: "file";
  depth: number;
  index: number;
  name: string;
}

export type TreeRow = DirRow | FileRow;

export type ChangeKind = "added" | "removed";

export interface CoreSpan {
  top: number;
  height: number;
  kind: ChangeKind;
}

export interface CoreCell {
  added: boolean;
  removed: boolean;
}

interface CoreExports {
  memory: WebAssembly.Memory;
  alloc(len: number): number;
  dealloc(ptr: number, len: number): void;
  tree_rows(ptr: number, len: number): bigint;
  change_cells(
    spans: number,
    count: number,
    content: number,
    track: number,
    cell: number,
  ): bigint;
}

const URL_PATH = "/deltoids_review.wasm";

let core: CoreExports | null = null;
let loading: Promise<void> | null = null;

// Instantiate a compiled module. Tests call this with the module built by
// crates/deltoids-review/build-wasm.sh.
export function initReviewCore(module: WebAssembly.Module): void {
  core = new WebAssembly.Instance(module, {}).exports as unknown as CoreExports;
}

async function compile(): Promise<WebAssembly.Module> {
  try {
    return await WebAssembly.compileStreaming(fetch(URL_PATH));
  } catch {
    const bytes = await (await fetch(URL_PATH)).arrayBuffer();
    return WebAssembly.compile(bytes);
  }
}

// Fetch and instantiate the module once. The fetch matches the index.html
// preload (as="fetch" + crossorigin), so it downloads once.
export function loadReviewCore(): Promise<void> {
  if (core) return Promise.resolve();
  loading ??= compile()
    .then(initReviewCore)
    .catch((err) => {
      loading = null;
      throw err;
    });
  return loading;
}

function exports(): CoreExports {
  if (!core) throw new Error("review core is not loaded");
  return core;
}

function take(c: CoreExports, packed: bigint): Uint8Array {
  const ptr = Number(packed >> 32n) >>> 0;
  const len = Number(packed & 0xffffffffn);
  const bytes = new Uint8Array(c.memory.buffer, ptr, len).slice();
  c.dealloc(ptr, len);
  return bytes;
}

// The file tree rows for `paths`, in display order. File rows carry the
// index of their path in `paths`.
export function treeRows(paths: string[]): TreeRow[] {
  const c = exports();
  const input = new TextEncoder().encode(paths.join("\0"));
  const ptr = c.alloc(input.length) >>> 0;
  new Uint8Array(c.memory.buffer, ptr, input.length).set(input);
  const json = new TextDecoder().decode(take(c, c.tree_rows(ptr, input.length)));
  c.dealloc(ptr, input.length);
  const raw = JSON.parse(json) as ([0, number, string] | [1, number, number, string])[];
  return raw.map((row) =>
    row[0] === 0
      ? { kind: "dir", depth: row[1], label: row[2] }
      : { kind: "file", depth: row[1], index: row[2], name: row[3] },
  );
}

// Which of the `track / cellPx` overview cells hold added or removed spans.
export function changeCells(
  spans: CoreSpan[],
  contentHeight: number,
  track: number,
  cellPx: number,
): CoreCell[] {
  const c = exports();
  const bytes = spans.length * 3 * 8;
  const ptr = c.alloc(bytes) >>> 0;
  const view = new Float64Array(c.memory.buffer, ptr, spans.length * 3);
  spans.forEach((span, i) => {
    view[i * 3] = span.top;
    view[i * 3 + 1] = span.height;
    view[i * 3 + 2] = span.kind === "added" ? 0 : 1;
  });
  const cells = take(c, c.change_cells(ptr, spans.length, contentHeight, track, cellPx));
  c.dealloc(ptr, bytes);
  return Array.from(cells, (bits) => ({
    added: (bits & 1) !== 0,
    removed: (bits & 2) !== 0,
  }));
}

import type { Sides } from "./github";
import {
  WASI,
  OpenFile,
  File as WasiFile,
  ConsoleStdout,
} from "./vendor/browser_wasi_shim.js";

// ---------------------------------------------------------------------------
// wasm engine
// ---------------------------------------------------------------------------

export interface Engine {
  // Render new-file lines start..=end (1-based, inclusive) of `after` as
  // highlighted context rows, revealing the lines a `.gap` divider skips.
  renderContext(
    after: string,
    path: string,
    start: number,
    end: number,
    theme: string,
  ): string;
  // Keep one file's diff alive so its hunks can grow and shrink context.
  openShape(sides: Sides): DiffShape;
}

// One file's diff with its current context shape. The engine owns the diff;
// a trap that replaces the engine instance reopens it and replays the steps,
// so a shape stays usable.
export interface DiffShape {
  render(theme: string): string;
  // Grow hunk `hunk` one scope level. False when it already shows the file.
  expand(hunk: number): boolean;
  // Undo the latest growth that reached hunk `hunk`. False when none did.
  shrink(hunk: number): boolean;
  close(): void;
}

interface Bound {
  renderContext: Engine["renderContext"];
  open(sides: Sides): number;
  render(id: number, theme: string): string;
  reshape(op: "expand" | "shrink", id: number, hunk: number): boolean;
  close(id: number): void;
}

interface EngineExports {
  memory: WebAssembly.Memory;
  alloc(len: number): number;
  dealloc(ptr: number, len: number): void;
  render_context(...args: number[]): bigint;
  session_open(...args: number[]): number;
  session_open_patch(...args: number[]): number;
  session_render(id: number, ...theme: number[]): bigint;
  session_expand(id: number, hunk: number): number;
  session_shrink(id: number, hunk: number): number;
  session_close(id: number): void;
}

// Compile the wasm module once. `fetch` reuses the `index.html` preload, so
// this runs on the main thread and the compiled module is posted to the
// review worker, which instantiates it.
let modulePromise: Promise<WebAssembly.Module> | null = null;

export function compileEngine(): Promise<WebAssembly.Module> {
  if (!modulePromise) modulePromise = compileModule();
  return modulePromise;
}

async function compileModule(): Promise<WebAssembly.Module> {
  try {
    return await WebAssembly.compileStreaming(fetch("/deltoids_wasm.wasm"));
  } catch {
    // Fall back when the server does not send application/wasm.
    const bytes = await (await fetch("/deltoids_wasm.wasm")).arrayBuffer();
    return WebAssembly.compile(bytes);
  }
}

// Instantiate the engine in the review worker. A Rust panic aborts as a wasm
// trap and can leave the instance's memory inconsistent, so a trap replaces
// the instance before the error is rethrown: only the call that trapped fails.
export async function instantiateEngine(
  module: WebAssembly.Module,
): Promise<Engine> {
  let current = bind(module);
  let generation = 0;
  const call = <T>(work: (bound: Bound) => T): T => {
    try {
      return work(current);
    } catch (err) {
      if (!(err instanceof WebAssembly.RuntimeError)) throw err;
      current = bind(module);
      generation++;
      throw new Error(`diff engine crashed (${err.message})`);
    }
  };
  const openShape = (sides: Sides): DiffShape => {
    const steps: ["expand" | "shrink", number][] = [];
    let id = 0;
    let openedIn = -1;
    const ensure = (bound: Bound): number => {
      if (openedIn !== generation) {
        id = bound.open(sides);
        openedIn = generation;
        for (const [op, hunk] of steps) bound.reshape(op, id, hunk);
      }
      return id;
    };
    const reshape = (op: "expand" | "shrink", hunk: number) =>
      call((bound) => {
        const changed = bound.reshape(op, ensure(bound), hunk);
        if (changed) steps.push([op, hunk]);
        return changed;
      });
    return {
      render: (theme) => call((bound) => bound.render(ensure(bound), theme)),
      expand: (hunk) => reshape("expand", hunk),
      shrink: (hunk) => reshape("shrink", hunk),
      close: () => {
        if (openedIn === generation) current.close(id);
        openedIn = -1;
      },
    };
  };
  return {
    renderContext: (...args) => call((bound) => bound.renderContext(...args)),
    openShape,
  };
}

function bind(module: WebAssembly.Module): Bound {
  const wasi = new WASI(
    [],
    [],
    [
      new OpenFile(new WasiFile([])),
      ConsoleStdout.lineBuffered((m) => console.log("[wasm]", m)),
      ConsoleStdout.lineBuffered((m) => console.warn("[wasm]", m)),
    ],
  );
  const importObject = {
    wasi_snapshot_preview1: wasi.wasiImport,
  } as unknown as WebAssembly.Imports;

  const instance = new WebAssembly.Instance(module, importObject);
  wasi.initialize(instance);

  const exports = instance.exports as unknown as EngineExports;
  const { memory, alloc, dealloc, render_context } = exports;
  const encoder = new TextEncoder();
  const decoder = new TextDecoder();

  function put(str: string): [number, number] {
    const bytes = encoder.encode(str);
    const ptr = alloc(bytes.length) >>> 0;
    new Uint8Array(memory.buffer, ptr, bytes.length).set(bytes);
    return [ptr, bytes.length];
  }

  // Read a packed `ptr << 32 | len` result into a string and free it.
  function takeResult(packed: bigint): string {
    const ptr = Number(packed >> 32n) >>> 0;
    const len = Number(packed & 0xffffffffn);
    const html = decoder.decode(
      new Uint8Array(memory.buffer, ptr, len).slice(),
    );
    dealloc(ptr, len);
    return html;
  }

  function renderContext(
    after: string,
    path: string,
    start: number,
    end: number,
    theme: string,
  ): string {
    const strs = [put(after), put(path), put(theme)];
    const html = takeResult(
      render_context(...strs.flat(), start >>> 0, end >>> 0),
    );
    for (const [p, l] of strs) dealloc(p, l);
    return html;
  }

  function withStrings<T>(strings: string[], work: (args: number[]) => T): T {
    const args = strings.map(put);
    try {
      return work(args.flat());
    } finally {
      for (const [p, l] of args) dealloc(p, l);
    }
  }

  return {
    renderContext,
    open: (sides) =>
      sides.kind === "patch"
        ? withStrings([sides.after, sides.patch, sides.path], (a) => exports.session_open_patch(...a))
        : withStrings([sides.before, sides.after, sides.path], (a) => exports.session_open(...a)),
    render: (id, theme) =>
      withStrings([theme], (a) => {
        const packed = exports.session_render(id, ...a);
        if (packed === 0n) throw new Error(`unknown diff session ${id}`);
        return takeResult(packed);
      }),
    reshape: (op, id, hunk) =>
      (op === "expand" ? exports.session_expand : exports.session_shrink)(id, hunk >>> 0) === 1,
    close: (id) => exports.session_close(id),
  };
}

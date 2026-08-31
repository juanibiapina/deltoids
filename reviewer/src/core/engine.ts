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
  // Compute the deltoids diff HTML body from full before/after content.
  // `theme` is a registry theme name; "" selects the default.
  renderFile(before: string, after: string, path: string, theme: string): string;
  // Compute the diff HTML from after content plus a unified patch, letting the
  // engine reconstruct the before side (one fewer GitHub request). `theme` is
  // a registry theme name; "" selects the default.
  renderFromPatch(
    after: string,
    patch: string,
    path: string,
    theme: string,
  ): string;
}

interface EngineExports {
  memory: WebAssembly.Memory;
  alloc(len: number): number;
  dealloc(ptr: number, len: number): void;
  render_file(...args: number[]): bigint;
  render_from_patch(...args: number[]): bigint;
}

// Lazily instantiated deltoids wasm module wrapped in a `renderFile` helper.
let enginePromise: Promise<Engine> | null = null;

export function loadEngine(): Promise<Engine> {
  if (!enginePromise) enginePromise = instantiateEngine();
  return enginePromise;
}

async function instantiateEngine(): Promise<Engine> {
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

  let instance: WebAssembly.Instance;
  try {
    const source = await WebAssembly.instantiateStreaming(
      fetch("/deltoids_wasm.wasm"),
      importObject,
    );
    instance = source.instance;
  } catch {
    // Fall back when the server does not send application/wasm.
    const bytes = await (await fetch("/deltoids_wasm.wasm")).arrayBuffer();
    const source = await WebAssembly.instantiate(bytes, importObject);
    instance = source.instance;
  }
  wasi.initialize(instance);

  const { memory, alloc, dealloc, render_file, render_from_patch } =
    instance.exports as unknown as EngineExports;
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

  function renderFile(
    before: string,
    after: string,
    path: string,
    theme: string,
  ): string {
    const args = [put(before), put(after), put(path), put(theme)];
    const html = takeResult(render_file(...args.flat()));
    for (const [p, l] of args) dealloc(p, l);
    return html;
  }

  function renderFromPatch(
    after: string,
    patch: string,
    path: string,
    theme: string,
  ): string {
    const args = [put(after), put(patch), put(path), put(theme)];
    const html = takeResult(render_from_patch(...args.flat()));
    for (const [p, l] of args) dealloc(p, l);
    return html;
  }

  return { renderFile, renderFromPatch };
}

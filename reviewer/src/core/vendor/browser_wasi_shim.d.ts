// Minimal ambient types for the vendored @bjorn3/browser_wasi_shim@0.4.2 build.
// Only the surface the engine loader touches is declared.

export class File {
  constructor(data: ArrayLike<number> | Uint8Array | number[]);
}

export class OpenFile {
  constructor(file: File);
}

export class ConsoleStdout {
  static lineBuffered(handler: (line: string) => void): ConsoleStdout;
}

export class WASI {
  constructor(args: string[], env: string[], fds: unknown[]);
  readonly wasiImport: Record<string, unknown>;
  initialize(instance: WebAssembly.Instance): void;
}

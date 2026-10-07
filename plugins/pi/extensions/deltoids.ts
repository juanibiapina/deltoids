/**
 * Overrides pi's edit, write, and bash tools so every file change lands in
 * a deltoids trace.
 *
 * - edit and write run the deltoids CLIs, which record their own diffs.
 * - bash runs through Kao (`kao run`), which captures every file the
 *   command changed; `deltoids record` imports that capture as one entry.
 *
 * Inside a Git working tree with Kao installed, edit and write also run
 * under `kao lock`, so they never overlap a captured bash command.
 * Elsewhere every tool runs as before, without capture.
 *
 * The `deltoids` binary must be installed and available on PATH. `kao`
 * is optional.
 */

import type { BashOperations, ExtensionAPI, ExtensionContext } from "@earendil-works/pi-coding-agent";
import {
  createBashToolDefinition,
  createEditToolDefinition,
  createLocalBashOperations,
  createWriteToolDefinition,
  getShellConfig,
  SettingsManager,
  withFileMutationQueue,
} from "@earendil-works/pi-coding-agent";
import { Type } from "@sinclair/typebox";
import type { ChildProcessWithoutNullStreams } from "node:child_process";
import { spawn, spawnSync } from "node:child_process";
import { createWriteStream } from "node:fs";
import { mkdtemp, rm, stat } from "node:fs/promises";
import { homedir, constants as osConstants, tmpdir } from "node:os";
import { isAbsolute, join, resolve } from "node:path";

interface ExternalEditInput {
  reason: string;
  path: string;
  oldText: string;
  newText: string;
}

interface ExternalEditSuccess {
  traceId?: string;
  diff?: string;
}

interface ExternalWriteInput {
  reason: string;
  path: string;
  content: string;
}

interface TraceState {
  traceId?: string;
}

/** Which pi session and tool call made a change. */
interface Origin {
  agent: "pi";
  sessionId: string;
  toolCallId: string;
}

interface RecordSuccess {
  recorded?: boolean;
  traceId?: string;
  paths?: string[];
}

interface CliOptions {
  /** Arguments after the trace id. */
  extraArgs?: string[];
  /** Run under `kao lock` so the call never overlaps a captured command. */
  locked?: boolean;
}

const externalEditSchema = Type.Object(
  {
    reason: Type.String({
      description: "Why this change is being made. One short sentence explaining the intent behind the edit.",
    }),
    path: Type.String({ description: "Path to the file to edit (relative or absolute)" }),
    oldText: Type.String({
      description:
        "Exact text to replace. It must match the file's current text exactly and appear exactly once.",
    }),
    newText: Type.String({ description: "Replacement text." }),
  },
  { additionalProperties: false },
);

const externalWriteSchema = Type.Object(
  {
    reason: Type.String({
      description: "Why this file is being written. One short sentence explaining the intent behind the write.",
    }),
    path: Type.String({ description: "Path to the file to write (relative or absolute)" }),
    content: Type.String({ description: "Content to write to the file" }),
  },
  { additionalProperties: false },
);

function resolveToolPath(cwd: string, filePath: string): string {
  const normalized = filePath.startsWith("@") ? filePath.slice(1) : filePath;
  if (normalized === "~") return homedir();
  if (normalized.startsWith("~/")) return `${homedir()}${normalized.slice(1)}`;
  return isAbsolute(normalized) ? normalized : resolve(cwd, normalized);
}

function fallbackReason(_path: string): string {
  return "Edit file";
}

function fallbackWriteReason(_path: string): string {
  return "Write file";
}

function prepareArguments(input: unknown): ExternalEditInput {
  if (!input || typeof input !== "object") return input as ExternalEditInput;

  const args = input as {
    reason?: unknown;
    path?: unknown;
  };
  const path = typeof args.path === "string" ? args.path : "file";
  const reason = typeof args.reason === "string" && args.reason.trim() ? args.reason : fallbackReason(path);

  return {
    ...(args as object),
    reason,
  } as ExternalEditInput;
}

function prepareWriteArguments(input: unknown): ExternalWriteInput {
  if (!input || typeof input !== "object") return input as ExternalWriteInput;

  const args = input as {
    reason?: unknown;
    path?: unknown;
    content?: unknown;
  };
  const path = typeof args.path === "string" ? args.path : "file";
  const reason = typeof args.reason === "string" && args.reason.trim() ? args.reason : fallbackWriteReason(path);

  return {
    ...(args as object),
    reason,
  } as ExternalWriteInput;
}

function tryParseJson(text: string): unknown {
  try {
    return JSON.parse(text);
  } catch {
    return undefined;
  }
}

function extractErrorMessage(value: unknown): string | undefined {
  if (!value || typeof value !== "object") return undefined;

  const record = value as Record<string, unknown>;
  const parts: string[] = [];
  const push = (item: unknown) => {
    if (typeof item !== "string") return;
    const text = item.trim();
    if (!text || parts.includes(text)) return;
    parts.push(text);
  };

  push(record.error);
  push(record.message);
  push(record.details);

  const errors = record.errors;
  if (Array.isArray(errors)) {
    for (const item of errors) push(item);
  }

  const traceId = typeof record.traceId === "string" ? record.traceId.trim() : "";
  if (traceId && !parts.some((part) => part.includes(traceId))) {
    parts.push(`Trace: ${traceId}`);
  }

  return parts.length > 0 ? parts.join("\n") : undefined;
}

function formatCliFailure(toolName: string, stderr: string, stdout: string, exitCode: number | null): string {
  const trimmedStderr = stderr.trim();
  const trimmedStdout = stdout.trim();

  if (trimmedStderr) {
    const parsed = tryParseJson(trimmedStderr);
    const parsedMessage = extractErrorMessage(parsed);
    if (parsedMessage) {
      return exitCode === null ? parsedMessage : `${parsedMessage} (exit ${exitCode})`;
    }
  }

  const parts: string[] = [];
  if (exitCode !== null) parts.push(`${toolName} failed with exit ${exitCode}`);
  if (trimmedStderr) parts.push(trimmedStderr);
  if (trimmedStdout) parts.push(trimmedStdout);
  return parts.join("\n\n") || `${toolName} failed`;
}

function isMissingTraceError(error: unknown): boolean {
  if (!(error instanceof Error)) return false;
  return /Trace does not exist|Invalid trace id/.test(error.message);
}

function getTraceIdFromDetails(details: unknown): string | undefined {
  if (!details || typeof details !== "object") return undefined;
  const traceId = (details as Record<string, unknown>).traceId;
  return typeof traceId === "string" && traceId.trim() ? traceId : undefined;
}

function rebuildTraceState(traceState: TraceState, ctx: ExtensionContext): void {
  traceState.traceId = undefined;

  const branch = ctx.sessionManager.getBranch();
  for (let index = branch.length - 1; index >= 0; index--) {
    const entry = branch[index];
    if (entry?.type !== "message") continue;
    const message = entry.message;
    if (
      message.role !== "toolResult" ||
      (message.toolName !== "edit" && message.toolName !== "write" && message.toolName !== "bash")
    )
      continue;
    const traceId = getTraceIdFromDetails(message.details);
    if (!traceId) continue;
    traceState.traceId = traceId;
    return;
  }
}

async function runExternalCli<T = ExternalEditSuccess>(
  command: string,
  payload: unknown,
  traceId: string | undefined,
  signal?: AbortSignal,
  options: CliOptions = {},
): Promise<T | undefined> {
  return new Promise<T | undefined>((resolvePromise, rejectPromise) => {
    const args = [command, ...(traceId ? [traceId] : []), ...(options.extraArgs ?? [])];
    const [bin, argv] = options.locked ? ["kao", ["lock", "--", "deltoids", ...args]] : ["deltoids", args];
    const child: ChildProcessWithoutNullStreams = spawn(bin, argv, {
      stdio: ["pipe", "pipe", "pipe"],
    });

    let stdout = "";
    let stderr = "";
    let settled = false;

    const cleanup = () => {
      if (signal) signal.removeEventListener("abort", onAbort);
    };

    const finish = (fn: () => void) => {
      if (settled) return;
      settled = true;
      cleanup();
      fn();
    };

    const onAbort = () => {
      child.kill("SIGTERM");
      finish(() => rejectPromise(new Error("Operation aborted")));
    };

    if (signal?.aborted) {
      onAbort();
      return;
    }

    signal?.addEventListener("abort", onAbort, { once: true });

    child.stdout.setEncoding("utf8");
    child.stderr.setEncoding("utf8");
    child.stdout.on("data", (chunk: string) => {
      stdout += chunk;
    });
    child.stderr.on("data", (chunk: string) => {
      stderr += chunk;
    });
    child.on("error", (error) => {
      finish(() => rejectPromise(error));
    });
    child.on("close", (code) => {
      if (code !== 0) {
        finish(() => rejectPromise(new Error(formatCliFailure(command, stderr, stdout, code))));
        return;
      }

      const trimmedStdout = stdout.trim();
      const parsed = trimmedStdout ? (tryParseJson(trimmedStdout) as T | undefined) : undefined;
      finish(() => resolvePromise(parsed));
    });

    child.stdin.write(JSON.stringify(payload));
    child.stdin.end();
  });
}

async function runExternalEdit(
  payload: ExternalEditInput & { origin?: Origin },
  traceId: string | undefined,
  signal?: AbortSignal,
  locked = false,
): Promise<ExternalEditSuccess | undefined> {
  return runExternalCli("edit", payload, traceId, signal, { locked });
}

async function runExternalWrite(
  payload: ExternalWriteInput & { origin?: Origin },
  traceId: string | undefined,
  signal?: AbortSignal,
  locked = false,
): Promise<ExternalEditSuccess | undefined> {
  return runExternalCli("write", payload, traceId, signal, { locked });
}

function originOf(ctx: ExtensionContext | undefined, toolCallId: string): Origin | undefined {
  const sessionId = ctx?.sessionManager.getSessionId();
  return sessionId ? { agent: "pi", sessionId, toolCallId } : undefined;
}

// ---------------------------------------------------------------------------
// Kao capture
// ---------------------------------------------------------------------------

/** Kao exits 125 when it never ran the command (or could not report it). */
const KAO_DID_NOT_RUN = 125;
/** How long to wait for the command's output to drain after Kao exits. */
const OUTPUT_DRAIN_MS = 200;

/** Checked on every call, so installing or removing Kao takes effect immediately. */
function hasKao(): boolean {
  return !spawnSync("kao", [], { stdio: "ignore" }).error;
}

function insideGitWorkTree(cwd: string): boolean {
  const result = spawnSync("git", ["rev-parse", "--is-inside-work-tree"], {
    cwd,
    encoding: "utf8",
    stdio: ["ignore", "pipe", "ignore"],
  });
  return result.status === 0 && result.stdout.trim() === "true";
}

/** Whether changes in `cwd` go through Kao: it is installed and `cwd` is in a Git working tree. */
function canCapture(cwd: string): boolean {
  return hasKao() && insideGitWorkTree(cwd);
}

/**
 * Bash operations that run the command through `kao run`, spooling the
 * capture archive Kao writes on file descriptor 3 into `archivePath`.
 *
 * Abort and timeout send SIGTERM to Kao alone: Kao forwards it to the
 * command, waits for it (escalating to SIGKILL), and still writes the
 * capture. Killing Kao's process group would lose the capture.
 *
 * When Kao reports it never ran the command, the command runs directly.
 */
function kaoBashOperations(archivePath: string, shellPath: string | undefined): BashOperations {
  return {
    exec: async (command, cwd, options) => {
      if (options.signal?.aborted) throw new Error("aborted");
      const result = await runUnderKao(command, cwd, archivePath, shellPath, options);
      if (result.ran) return { exitCode: result.exitCode };
      return createLocalBashOperations({ shellPath }).exec(command, cwd, options);
    },
  };
}

/** The bash settings pi applies to its own bash tool. */
function bashSettings(cwd: string): { commandPrefix?: string; shellPath?: string } {
  const settings = SettingsManager.create(cwd);
  return { commandPrefix: settings.getShellCommandPrefix(), shellPath: settings.getShellPath() };
}

type ExecOptions = Parameters<BashOperations["exec"]>[2];

function runUnderKao(
  command: string,
  cwd: string,
  archivePath: string,
  shellPath: string | undefined,
  { onData, signal, timeout, env }: ExecOptions,
): Promise<{ ran: boolean; exitCode: number | null }> {
  const shell = getShellConfig(shellPath);
  const viaStdin = shell.commandTransport === "stdin";
  const args = ["run", "--", shell.shell, ...shell.args, ...(viaStdin ? [] : [command])];
  return new Promise((resolvePromise, rejectPromise) => {
    const child = spawn("kao", args, {
      cwd,
      env,
      detached: process.platform !== "win32",
      stdio: [viaStdin ? "pipe" : "ignore", "pipe", "pipe", "pipe"],
    });
    if (viaStdin) {
      child.stdin?.on("error", () => {});
      child.stdin?.end(command);
    }
    const archive = createWriteStream(archivePath);
    const archiveDone = new Promise<void>((done) => archive.on("close", () => done()));
    child.stdio[3]?.pipe(archive as NodeJS.WritableStream);
    child.stdout?.on("data", onData);
    child.stderr?.on("data", onData);

    let stopped: "aborted" | "timeout" | undefined;
    const stop = (reason: "aborted" | "timeout") => {
      stopped ??= reason;
      if (child.pid) {
        try {
          process.kill(child.pid, "SIGTERM");
        } catch {
          // Kao already exited.
        }
      }
    };
    const onAbort = () => stop("aborted");
    signal?.addEventListener("abort", onAbort, { once: true });
    const timer = timeout && timeout > 0 ? setTimeout(() => stop("timeout"), timeout * 1000) : undefined;

    child.on("error", (error: NodeJS.ErrnoException) => {
      cleanup();
      archive.destroy();
      if (error.code === "ENOENT") resolvePromise({ ran: false, exitCode: null });
      else rejectPromise(error);
    });
    child.on("exit", async (code, signalName) => {
      cleanup();
      await Promise.race([drained(child), delay(OUTPUT_DRAIN_MS)]);
      await archiveDone;
      const size = await stat(archivePath).then((info) => info.size, () => 0);
      if (code === KAO_DID_NOT_RUN && size === 0 && !stopped) {
        resolvePromise({ ran: false, exitCode: null });
        return;
      }
      if (stopped === "aborted") rejectPromise(new Error("aborted"));
      else if (stopped === "timeout") rejectPromise(new Error(`timeout:${timeout}`));
      else resolvePromise({ ran: true, exitCode: code ?? (signalName ? 128 + signalNumber(signalName) : 1) });
    });

    function cleanup() {
      if (timer) clearTimeout(timer);
      signal?.removeEventListener("abort", onAbort);
    }
  });
}

function drained(child: ReturnType<typeof spawn>): Promise<void> {
  const ended = (stream: NodeJS.ReadableStream | null) =>
    new Promise<void>((done) => (stream ? stream.once("close", () => done()) : done()));
  return Promise.all([ended(child.stdout), ended(child.stderr)]).then(() => undefined);
}

function delay(ms: number): Promise<void> {
  return new Promise((done) => setTimeout(done, ms));
}

function signalNumber(name: NodeJS.Signals): number {
  return (osConstants.signals as Record<string, number>)[name] ?? 0;
}

/**
 * Import the capture at `archivePath` into the session's trace and say
 * what was recorded. Returns undefined when nothing changed.
 */
async function recordCapture(
  archivePath: string,
  command: string,
  origin: Origin | undefined,
  traceState: TraceState,
): Promise<{ text: string; traceId?: string } | undefined> {
  const size = await stat(archivePath).then((info) => info.size, () => 0);
  if (size === 0) return undefined;
  const payload = { tool: "bash", command, ...(origin ? { origin } : {}) };
  const record = (traceId: string | undefined) =>
    runExternalCli<RecordSuccess>("record", payload, traceId, undefined, {
      extraArgs: ["--capture", archivePath],
    });
  try {
    let result: RecordSuccess | undefined;
    try {
      result = await record(traceState.traceId);
    } catch (error) {
      if (!traceState.traceId || !isMissingTraceError(error)) throw error;
      traceState.traceId = undefined;
      result = await record(undefined);
    }
    if (!result?.recorded || !result.traceId) return undefined;
    traceState.traceId = result.traceId;
    const count = result.paths?.length ?? 0;
    const noun = count === 1 ? "file" : "files";
    return { text: `Recorded ${count} changed ${noun}. Trace: ${result.traceId}.`, traceId: result.traceId };
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    return { text: `Recording file changes failed: ${message}\nCapture kept at ${archivePath}.` };
  }
}

export default function (pi: ExtensionAPI) {
  const builtinEdit = createEditToolDefinition(process.cwd());
  const builtinWrite = createWriteToolDefinition(process.cwd());
  const traceState: TraceState = {};

  pi.on("session_start", async (_event, ctx) => {
    rebuildTraceState(traceState, ctx);
  });

  pi.on("session_tree", async (_event, ctx) => {
    rebuildTraceState(traceState, ctx);
  });

  pi.on("session_shutdown", async () => {
    traceState.traceId = undefined;
  });

  pi.registerTool({
    ...builtinEdit,
    renderCall: undefined,
    renderResult: undefined,
    renderShell: undefined,
    description:
      "Replace one exact region of a file. `oldText` must match the file's current text exactly and appear exactly once. To make several changes, call `edit` once per change; each call matches against the file's current text, so target text as it exists after any earlier edit. Provide a reason explaining why the change is being made.",
    promptGuidelines: [
      ...(builtinEdit.promptGuidelines ?? []),
      "Include reason: why this change is being made.",
      "To make several changes, issue several `edit` calls, each replacing one exact region.",
    ],
    parameters: externalEditSchema,
    prepareArguments,
    async execute(toolCallId, params: ExternalEditInput, signal, _onUpdate, ctx) {
      const resolvedPath = resolveToolPath(ctx.cwd, params.path);

      return withFileMutationQueue(resolvedPath, async () => {
        const payload = {
          reason: params.reason,
          path: resolvedPath,
          oldText: params.oldText,
          newText: params.newText,
          origin: originOf(ctx, toolCallId),
        };
        const locked = canCapture(process.cwd());
        const initialTraceId = traceState.traceId;
        let result: ExternalEditSuccess | undefined;
        try {
          result = await runExternalEdit(payload, initialTraceId, signal, locked);
        } catch (error) {
          if (initialTraceId && isMissingTraceError(error)) {
            traceState.traceId = undefined;
            result = await runExternalEdit(payload, undefined, signal, locked);
          } else {
            throw error;
          }
        }
        const resultTraceId = result?.traceId?.trim() || traceState.traceId;
        if (resultTraceId) traceState.traceId = resultTraceId;

        const traceSuffix = resultTraceId ? ` Trace: ${resultTraceId}.` : "";
        const details: Record<string, unknown> = {};
        if (result?.diff) details.diff = result.diff;
        if (resultTraceId) details.traceId = resultTraceId;
        return {
          content: [{ type: "text", text: `Edited ${params.path}.${traceSuffix}` }],
          details: Object.keys(details).length > 0 ? details : undefined,
        };
      });
    },
  });

  pi.registerTool({
    ...builtinWrite,
    renderCall: undefined,
    renderResult: undefined,
    description:
      "Write content to a file. Creates the file if it doesn't exist, overwrites if it does. Automatically creates parent directories. Provide a reason explaining why the file is being written.",
    promptGuidelines: [
      ...(builtinWrite.promptGuidelines ?? []),
      "Include reason: why the file is being written.",
    ],
    parameters: externalWriteSchema,
    prepareArguments: prepareWriteArguments,
    async execute(toolCallId, params: ExternalWriteInput, signal, _onUpdate, ctx) {
      const resolvedPath = resolveToolPath(ctx.cwd, params.path);

      return withFileMutationQueue(resolvedPath, async () => {
        const payload = {
          reason: params.reason,
          path: resolvedPath,
          content: params.content,
          origin: originOf(ctx, toolCallId),
        };
        const locked = canCapture(process.cwd());
        const initialTraceId = traceState.traceId;
        let result: ExternalEditSuccess | undefined;
        try {
          result = await runExternalWrite(payload, initialTraceId, signal, locked);
        } catch (error) {
          if (initialTraceId && isMissingTraceError(error)) {
            traceState.traceId = undefined;
            result = await runExternalWrite(payload, undefined, signal, locked);
          } else {
            throw error;
          }
        }
        const resultTraceId = result?.traceId?.trim() || traceState.traceId;
        if (resultTraceId) traceState.traceId = resultTraceId;

        const traceSuffix = resultTraceId ? ` Trace: ${resultTraceId}.` : "";
        const details: Record<string, unknown> = {};
        if (result?.diff) details.diff = result.diff;
        if (resultTraceId) details.traceId = resultTraceId;
        if (resolvedPath) details.path = params.path;
        return {
          content: [{ type: "text", text: `Wrote ${params.path}.${traceSuffix}` }],
          details: Object.keys(details).length > 0 ? details : undefined,
        };
      });
    },
  });

  const builtinBash = createBashToolDefinition(process.cwd());
  pi.registerTool({
    ...builtinBash,
    async execute(toolCallId, params, signal, onUpdate, ctx) {
      const cwd = ctx?.cwd || process.cwd();
      const settings = bashSettings(cwd);
      if (!canCapture(cwd)) {
        return createBashToolDefinition(cwd, settings).execute(toolCallId, params, signal, onUpdate, ctx);
      }

      const dir = await mkdtemp(join(tmpdir(), "deltoids-capture-"));
      const archivePath = join(dir, "capture.tar");
      const captured = createBashToolDefinition(cwd, {
        ...settings,
        operations: kaoBashOperations(archivePath, settings.shellPath),
      });
      let result: Awaited<ReturnType<typeof builtinBash.execute>> | undefined;
      let failure: unknown;
      try {
        result = await captured.execute(toolCallId, params, signal, onUpdate, ctx);
      } catch (error) {
        failure = error;
      }
      const note = await recordCapture(archivePath, params.command, originOf(ctx, toolCallId), traceState);
      if (!note || note.traceId) await rm(dir, { recursive: true, force: true });

      if (failure !== undefined) {
        if (note && failure instanceof Error) throw new Error(`${failure.message}\n\n${note.text}`);
        throw failure;
      }
      if (!result || !note) return result!;
      return {
        ...result,
        content: [...result.content, { type: "text" as const, text: note.text }],
        details: { ...(result.details ?? {}), ...(note.traceId ? { traceId: note.traceId } : {}) },
      };
    },
  });
}

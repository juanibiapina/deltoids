/**
 * Edit/Write extension — override pi's edit and write tools with external
 * CLIs.
 *
 * Two self-contained tool sets, selected once at extension load by the
 * `DELTOIDS_EDIT_MODE` env var. The mode is fixed for the lifetime of the
 * pi session — changing it requires restarting pi. This keeps the system
 * prompt static (so prompt caching stays warm across turns) and the tool
 * surface deterministic.
 *
 *   - `text` (default): overrides pi's built-in `edit` with a
 *     `deltoids edit` pass-through (`oldText`/`newText` replacement).
 *     Pi's built-in `read` is used for everything. `write` is also
 *     overridden for trace recording.
 *
 *   - `hash`: overrides pi's built-in `edit` with a `deltoids hashedit`
 *     pass-through that takes line+hash anchors. The tool the model sees
 *     is still called `edit`, but its schema is the line-anchored shape.
 *     Adds `hashread` as a new tool for reading text files with anchors,
 *     and **overrides the built-in `read` tool's description** so the
 *     model is told that text-file reads should go through `hashread`.
 *     The `read` execute path is unchanged — it still handles images,
 *     directories, URLs, archives, SQLite, and other non-text content
 *     `hashread` cannot handle. `write` is overridden as in text mode.
 *
 * Requirements:
 *   - `deltoids` must be installed and available on PATH.
 */

import type { ExtensionAPI, ExtensionContext } from "@mariozechner/pi-coding-agent";
import { createEditToolDefinition, createReadToolDefinition, createWriteToolDefinition, withFileMutationQueue } from "@mariozechner/pi-coding-agent";
import { Type } from "@sinclair/typebox";
import type { ChildProcessWithoutNullStreams } from "node:child_process";
import { spawn } from "node:child_process";
import { homedir } from "node:os";
import { isAbsolute, resolve } from "node:path";

interface ExternalEditInput {
  reason: string;
  path: string;
  edits: Array<{
    reason: string;
    oldText: string;
    newText: string;
  }>;
}

interface ExternalEditSuccess {
  ok?: boolean;
  path?: string;
  traceId?: string;
  diff?: string;
  message?: string;
}

interface ExternalWriteInput {
  reason: string;
  path: string;
  content: string;
}

interface ExternalHashEditOp {
  op: "replace" | "insert_before" | "insert_after" | "delete";
  reason: string;
  pos: string;
  end?: string;
  lines?: string[];
}

interface ExternalHashEditInput {
  reason: string;
  path: string;
  edits: ExternalHashEditOp[];
}

interface ExternalHashReadInput {
  path: string;
  offset?: number;
  limit?: number;
}

type EditMode = "text" | "hash";

/** Resolve the active edit mode from `DELTOIDS_EDIT_MODE` once at
 * extension load. The mode is fixed for the lifetime of the pi
 * session — changing it requires restarting pi. This keeps the system
 * prompt static (cache-friendly) and the tool surface deterministic
 * (one self-contained set per mode).
 *
 * - `text` (default): override pi's built-in `edit` with a
 *   `deltoids edit` pass-through (`oldText`/`newText`). Pi's built-in
 *   `read` is used for everything.
 * - `hash`: override pi's built-in `edit` with a `deltoids hashedit`
 *   pass-through (line+hash anchors). The model still sees the tool as
 *   `edit`; only the schema and backend differ. Also register `hashread`
 *   so the model can obtain the `LINEhh` anchors `edit` requires, and
 *   override the built-in `read` tool's description (execute path
 *   unchanged) so the model is steered away from `read` for text files
 *   — reading text with `read` would force a re-read through `hashread`
 *   before editing, wasting a turn. `read` remains the right choice for
 *   images, directories, URLs, archives, SQLite, and other non-text
 *   content `hashread` cannot handle.
 */
function resolveEditMode(): EditMode {
  const raw = (process.env.DELTOIDS_EDIT_MODE ?? "").trim().toLowerCase();
  return raw === "hash" ? "hash" : "text";
}

interface TraceState {
  traceId?: string;
}

const externalEditSchema = Type.Object(
  {
    reason: Type.String({
      description: "Why this change is being made. One short sentence explaining the intent behind the overall edit.",
    }),
    path: Type.String({ description: "Path to the file to edit (relative or absolute)" }),
    edits: Type.Array(
      Type.Object(
        {
          reason: Type.String({
            description: "Why this specific replacement is being made. One short sentence explaining the intent behind this edit block.",
          }),
          oldText: Type.String({
            description:
              "Exact text for one targeted replacement. It must be unique in the original file and must not overlap with any other edits[].oldText in the same call.",
          }),
          newText: Type.String({ description: "Replacement text for this targeted edit." }),
        },
        { additionalProperties: false },
      ),
      {
        description:
          "One or more targeted replacements. Each edit is matched against the original file, not incrementally. Do not include overlapping or nested edits. If two changes touch the same block or nearby lines, merge them into one edit instead.",
      },
    ),
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

const hashEditReplaceSchema = Type.Object(
  {
    op: Type.Literal("replace"),
    reason: Type.String({ description: "Why this specific replacement is being made." }),
    pos: Type.String({ description: "Anchor (LINEhh) from hashread output, e.g. \"42sr\"." }),
    end: Type.Optional(
      Type.String({ description: "Inclusive end anchor for a range replace." }),
    ),
    lines: Type.Optional(Type.Array(Type.String(), { description: "Replacement lines." })),
  },
  { additionalProperties: false },
);

const hashEditInsertBeforeSchema = Type.Object(
  {
    op: Type.Literal("insert_before"),
    reason: Type.String({ description: "Why this insert is being made." }),
    pos: Type.String({ description: "Anchor (LINEhh) or \"BOF\"." }),
    lines: Type.Array(Type.String(), { description: "Lines to insert before pos." }),
  },
  { additionalProperties: false },
);

const hashEditInsertAfterSchema = Type.Object(
  {
    op: Type.Literal("insert_after"),
    reason: Type.String({ description: "Why this insert is being made." }),
    pos: Type.String({ description: "Anchor (LINEhh) or \"EOF\"." }),
    lines: Type.Array(Type.String(), { description: "Lines to insert after pos." }),
  },
  { additionalProperties: false },
);

const hashEditDeleteSchema = Type.Object(
  {
    op: Type.Literal("delete"),
    reason: Type.String({ description: "Why this delete is being made." }),
    pos: Type.String({ description: "Anchor (LINEhh) to delete." }),
    end: Type.Optional(
      Type.String({ description: "Inclusive end anchor for a range delete." }),
    ),
  },
  { additionalProperties: false },
);

const hashEditSchema = Type.Object(
  {
    reason: Type.String({
      description: "Why this change is being made overall.",
    }),
    path: Type.String({ description: "Path to the file to edit (relative or absolute)." }),
    edits: Type.Array(
      Type.Union([
        hashEditReplaceSchema,
        hashEditInsertBeforeSchema,
        hashEditInsertAfterSchema,
        hashEditDeleteSchema,
      ]),
      {
        description:
          "One or more hashline ops. Each op references lines by their LINEhh anchor copied verbatim from the latest hashread output. Anchors are validated against the current file before any change \u2014 a single stale anchor rejects the whole batch.",
      },
    ),
  },
  { additionalProperties: false },
);

const hashReadSchema = Type.Object(
  {
    path: Type.String({ description: "Path to the file to read (relative or absolute)." }),
    offset: Type.Optional(
      Type.Integer({ minimum: 1, description: "1-indexed first line to return." }),
    ),
    limit: Type.Optional(
      Type.Integer({ minimum: 1, description: "Maximum number of lines to return." }),
    ),
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

function fallbackEditReason(_path: string): string {
  return "Edit block";
}

function fallbackWriteReason(_path: string): string {
  return "Write file";
}

function prepareArguments(input: unknown): ExternalEditInput {
  if (!input || typeof input !== "object") return input as ExternalEditInput;

  const args = input as {
    reason?: unknown;
    path?: unknown;
    edits?: unknown;
    oldText?: unknown;
    newText?: unknown;
  };
  const path = typeof args.path === "string" ? args.path : "file";
  const reason = typeof args.reason === "string" && args.reason.trim() ? args.reason : fallbackReason(path);

  let edits = Array.isArray(args.edits) ? args.edits : [];
  if (typeof args.oldText === "string" && typeof args.newText === "string") {
    edits = [...edits, { oldText: args.oldText, newText: args.newText }];
  }

  return {
    ...(args as object),
    reason,
    edits: edits.map((edit) => {
      const record = (edit && typeof edit === "object" ? edit : {}) as Record<string, unknown>;
      return {
        ...record,
        reason:
          typeof record.reason === "string" && record.reason.trim()
            ? record.reason
            : fallbackEditReason(path),
      };
    }),
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
      (message.toolName !== "edit" &&
        message.toolName !== "write" &&
        message.toolName !== "hashedit")
    )
      continue;
    const traceId = getTraceIdFromDetails(message.details);
    if (!traceId) continue;
    traceState.traceId = traceId;
    return;
  }
}

async function runExternalCli(
  command: string,
  payload: unknown,
  traceId: string | undefined,
  signal?: AbortSignal,
): Promise<ExternalEditSuccess | undefined> {
  return new Promise<ExternalEditSuccess | undefined>((resolvePromise, rejectPromise) => {
    const args = traceId ? [command, traceId] : [command];
    const child: ChildProcessWithoutNullStreams = spawn("deltoids", args, {
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
      const parsed = trimmedStdout ? (tryParseJson(trimmedStdout) as ExternalEditSuccess | undefined) : undefined;
      finish(() => resolvePromise(parsed));
    });

    child.stdin.write(JSON.stringify(payload));
    child.stdin.end();
  });
}

async function runExternalEdit(
  payload: ExternalEditInput,
  traceId: string | undefined,
  signal?: AbortSignal,
): Promise<ExternalEditSuccess | undefined> {
  return runExternalCli("edit", payload, traceId, signal);
}

async function runExternalWrite(
  payload: ExternalWriteInput,
  traceId: string | undefined,
  signal?: AbortSignal,
): Promise<ExternalEditSuccess | undefined> {
  return runExternalCli("write", payload, traceId, signal);
}

async function runExternalHashEdit(
  payload: ExternalHashEditInput,
  traceId: string | undefined,
  signal?: AbortSignal,
): Promise<ExternalEditSuccess | undefined> {
  return runExternalCli("hashedit", payload, traceId, signal);
}

async function runExternalHashRead(
  payload: ExternalHashReadInput,
  signal?: AbortSignal,
): Promise<string> {
  return new Promise<string>((resolvePromise, rejectPromise) => {
    const child: ChildProcessWithoutNullStreams = spawn("deltoids", ["hashread"], {
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
        finish(() => rejectPromise(new Error(formatCliFailure("hashread", stderr, stdout, code))));
        return;
      }
      finish(() => resolvePromise(stdout));
    });

    child.stdin.write(JSON.stringify(payload));
    child.stdin.end();
  });
}

export default function (pi: ExtensionAPI) {
  const builtinEdit = createEditToolDefinition(process.cwd());
  const builtinWrite = createWriteToolDefinition(process.cwd());
  const builtinRead = createReadToolDefinition(process.cwd());
  const traceState: TraceState = {};

  const mode = resolveEditMode();

  pi.on("session_start", async (_event, ctx) => {
    rebuildTraceState(traceState, ctx);
    if (ctx.hasUI) ctx.ui.setStatus("deltoids-mode", `deltoids: ${mode}`);
  });

  pi.on("session_tree", async (_event, ctx) => {
    rebuildTraceState(traceState, ctx);
  });

  pi.on("session_shutdown", async () => {
    traceState.traceId = undefined;
  });

  if (mode === "text") {
    pi.registerTool({
      ...builtinEdit,
      description:
        "Edit a single file using exact text replacement. Provide a reason for the overall change and for each edit block, explaining why the change is being made.",
      promptGuidelines: [
        ...(builtinEdit.promptGuidelines ?? []),
        "Include reason: why the overall file change is being made.",
        "Include edits[].reason: why each replacement block is being made.",
      ],
      parameters: externalEditSchema,
      prepareArguments,
      async execute(_toolCallId, params: ExternalEditInput, signal, _onUpdate, ctx) {
        const resolvedPath = resolveToolPath(ctx.cwd, params.path);

        return withFileMutationQueue(resolvedPath, async () => {
          const payload = {
            reason: params.reason,
            path: resolvedPath,
            edits: params.edits,
          };
          const initialTraceId = traceState.traceId;
          let result: ExternalEditSuccess | undefined;
          try {
            result = await runExternalEdit(payload, initialTraceId, signal);
          } catch (error) {
            if (initialTraceId && isMissingTraceError(error)) {
              traceState.traceId = undefined;
              result = await runExternalEdit(payload, undefined, signal);
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
  } else {
    pi.registerTool({
      name: "hashread",
      description:
        "Read a text file with hashline anchors. **Use this in place of the built-in `read` tool when reading text files** — the `edit` tool in this session requires the `LINEhh` anchors that only `hashread` produces. The built-in `read` is still the right choice for images, directories, URLs, archives, SQLite, and other non-text content. Each line is returned as `LINEhh|TEXT` where `LINEhh` is the anchor token (1-indexed line number plus a 2-character content hash). Copy the `LINEhh` token verbatim into `pos`/`end` fields of `edit` ops. Never include the `|TEXT` body in those fields.",
      promptGuidelines: [
        "Use `hashread`, not the built-in `read`, for any text file — even one-off inspection reads. Reading text with `read` first wastes a turn because the `edit` tool needs `LINEhh` anchors only `hashread` produces.",
        "The built-in `read` tool remains appropriate for images, directories, URLs, archives, SQLite, and other non-text content `hashread` cannot handle.",
      ],
      parameters: hashReadSchema,
      async execute(_toolCallId, params: ExternalHashReadInput, signal, _onUpdate, ctx) {
        const resolvedPath = resolveToolPath(ctx.cwd, params.path);
        const payload: ExternalHashReadInput = {
          path: resolvedPath,
          ...(params.offset !== undefined ? { offset: params.offset } : {}),
          ...(params.limit !== undefined ? { limit: params.limit } : {}),
        };
        const body = await runExternalHashRead(payload, signal);
        return {
          content: [{ type: "text", text: body.endsWith("\n") ? body : `${body}\n` }],
          details: { path: params.path },
        };
      },
    });

    pi.registerTool({
      ...builtinRead,
      description:
        "Read a file. **In this session, use `hashread` instead for any text file** — even one-off inspection reads. The `edit` tool requires `LINEhh` anchors only `hashread` produces, so reading text with `read` first forces a re-read through `hashread` before you can edit, wasting a turn. Use this `read` tool only when `hashread` cannot handle the content: images (jpg, png, gif, webp), directories, URLs, archives, SQLite, and other non-text content. Images are sent as attachments.",
      promptGuidelines: [
        "Prefer `hashread` over `read` for every text file, including quick peeks. `read` is reserved for non-text content (images, directories, archives, SQLite, etc.).",
      ],
    });

    pi.registerTool({
      ...builtinEdit,
      description:
        "Edit a text file via line+hash anchors obtained from `hashread`. First call `hashread` to obtain `LINEhh` anchors, then issue ops referencing those anchors. Ops: `replace`, `insert_before`, `insert_after`, `delete`. Anchors are validated against the current file before any change; one stale anchor rejects the whole batch and the file is untouched. On stale anchor, the error reprints the affected region with fresh anchors so you can retry without re-reading.",
      promptGuidelines: [
        "First call `hashread` to obtain LINEhh anchors. Copy the LINEhh token (e.g. \"42sr\") verbatim into pos/end.",
        "Each op carries its own reason. Be specific about why each block changes.",
        "Use insert_before with pos=\"BOF\" to prepend, insert_after with pos=\"EOF\" to append.",
        "Prefer narrow ops: replace for in-place changes, insert_before/insert_after to add, delete to remove.",
        "Never shift anchors for prior ops; the engine applies them bottom-up against the original file.",
      ],
      parameters: hashEditSchema,
      async execute(_toolCallId, params: ExternalHashEditInput, signal, _onUpdate, ctx) {
        const resolvedPath = resolveToolPath(ctx.cwd, params.path);

        return withFileMutationQueue(resolvedPath, async () => {
          const payload: ExternalHashEditInput = {
            reason: params.reason,
            path: resolvedPath,
            edits: params.edits,
          };
          const initialTraceId = traceState.traceId;
          let result: ExternalEditSuccess | undefined;
          try {
            result = await runExternalHashEdit(payload, initialTraceId, signal);
          } catch (error) {
            if (initialTraceId && isMissingTraceError(error)) {
              traceState.traceId = undefined;
              result = await runExternalHashEdit(payload, undefined, signal);
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
  }

  pi.registerTool({
    ...builtinWrite,
    description:
      "Write content to a file. Creates the file if it doesn't exist, overwrites if it does. Automatically creates parent directories. Provide a reason explaining why the file is being written.",
    promptGuidelines: [
      ...(builtinWrite.promptGuidelines ?? []),
      "Include reason: why the file is being written.",
    ],
    parameters: externalWriteSchema,
    prepareArguments: prepareWriteArguments,
    async execute(_toolCallId, params: ExternalWriteInput, signal, _onUpdate, ctx) {
      const resolvedPath = resolveToolPath(ctx.cwd, params.path);

      return withFileMutationQueue(resolvedPath, async () => {
        const payload = {
          reason: params.reason,
          path: resolvedPath,
          content: params.content,
        };
        const initialTraceId = traceState.traceId;
        let result: ExternalEditSuccess | undefined;
        try {
          result = await runExternalWrite(payload, initialTraceId, signal);
        } catch (error) {
          if (initialTraceId && isMissingTraceError(error)) {
            traceState.traceId = undefined;
            result = await runExternalWrite(payload, undefined, signal);
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
}

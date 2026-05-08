/**
 * Edit/Write extension - override pi's edit and write tools with external CLIs
 *
 * Requirements:
 *   - `edit` must be installed and available on PATH
 *   - `write` must be installed and available on PATH
 */

import type { ExtensionAPI, ExtensionContext } from "@mariozechner/pi-coding-agent";
import { createEditToolDefinition, createWriteToolDefinition, withFileMutationQueue } from "@mariozechner/pi-coding-agent";
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
    if (message.role !== "toolResult" || (message.toolName !== "edit" && message.toolName !== "write")) continue;
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

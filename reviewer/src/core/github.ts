import { looksBinary, type PrRef } from "./lib";
import type { Engine } from "./engine";

// ---------------------------------------------------------------------------
// GitHub API client
// ---------------------------------------------------------------------------

const TOKEN_KEY = "deltoids.gh.token";

export function token(): string {
  return localStorage.getItem(TOKEN_KEY) || "";
}

export function setToken(value: string): void {
  if (value.trim()) localStorage.setItem(TOKEN_KEY, value.trim());
  else localStorage.removeItem(TOKEN_KEY);
}

const JSON_MEDIA = "application/vnd.github+json";
// The raw media type returns the file body directly and works up to 100 MB;
// the JSON form leaves `content` empty for files over 1 MB. GitHub gives both
// forms the same ETag, so a browser cache holding the JSON form revalidates
// it with a 304 and serves the JSON body for a raw request (seen in
// Firefox). Raw requests therefore bypass the HTTP cache.
const RAW_MEDIA = "application/vnd.github.raw+json";

function ghHeaders(auth: string, accept: string): Record<string, string> {
  const headers: Record<string, string> = { Accept: accept };
  if (auth) headers.Authorization = `Bearer ${auth}`;
  return headers;
}

interface GhError extends Error {
  detail?: string;
}

export class RateLimitError extends Error {}

let lastRemaining: number | null = null;

export function rateRemaining(): number | null {
  return lastRemaining;
}

async function ghFetch(
  url: string,
  auth: string,
  accept = JSON_MEDIA,
): Promise<Response> {
  const res = await fetch(url, {
    headers: ghHeaders(auth, accept),
    cache: accept === RAW_MEDIA ? "no-store" : "default",
  });
  const remaining = res.headers.get("x-ratelimit-remaining");
  if (remaining !== null && Number.isFinite(Number(remaining))) {
    lastRemaining = Number(remaining);
  }
  return res;
}

function isRateLimited(res: Response): boolean {
  return (
    res.status === 429 ||
    (res.status === 403 && res.headers.get("x-ratelimit-remaining") === "0")
  );
}

async function gh<T>(url: string): Promise<{ data: T; remaining: string | null }> {
  const res = await ghFetch(url, token());
  const remaining = res.headers.get("x-ratelimit-remaining");
  if (!res.ok) {
    const body = await res.text();
    let message = `${res.status} ${res.statusText}`;
    if (isRateLimited(res)) {
      message = token()
        ? "GitHub rate limit reached for this token."
        : "GitHub rate limit reached (60/hr). Add a token with 🔑 for 5000/hr.";
    } else if (res.status === 404) {
      message = "Not found. Private repo? Add a token with 🔑.";
    }
    const err: GhError = new Error(message);
    err.detail = body;
    throw err;
  }
  return { data: (await res.json()) as T, remaining };
}

export interface Pr {
  number: number;
  title: string;
  additions: number;
  deletions: number;
  base: { sha: string };
  head: { sha: string };
}

export interface PrFile {
  filename: string;
  previous_filename?: string;
  status: string;
  patch?: string;
  additions?: number;
  deletions?: number;
  // Blob sha of the file at the PR head. Content-addressed, so it changes iff
  // the file's content changes — used to reset a file's "reviewed" mark when a
  // new commit touches it. Always present on the `/pulls/{n}/files` response.
  sha?: string;
}

export async function fetchPr(ref: PrRef): Promise<Pr> {
  const { data } = await gh<Pr>(
    `https://api.github.com/repos/${ref.owner}/${ref.repo}/pulls/${ref.number}`,
  );
  return data;
}

export async function fetchFiles(ref: PrRef): Promise<PrFile[]> {
  const files: PrFile[] = [];
  for (let page = 1; page <= 30; page++) {
    const { data } = await gh<PrFile[]>(
      `https://api.github.com/repos/${ref.owner}/${ref.repo}/pulls/${ref.number}/files?per_page=100&page=${page}`,
    );
    files.push(...data);
    if (data.length < 100) break;
  }
  return files;
}

// Fetch a file's UTF-8 content at a ref, or "" when it does not exist there.
async function fetchContent(
  repoRef: PrRef,
  path: string,
  ref: string,
  auth: string,
): Promise<string> {
  const url = `https://api.github.com/repos/${repoRef.owner}/${repoRef.repo}/contents/${encodeURIComponent(
    path,
  ).replace(/%2F/g, "/")}?ref=${encodeURIComponent(ref)}`;
  const res = await ghFetch(url, auth, RAW_MEDIA);
  if (res.status === 404) return "";
  if (isRateLimited(res)) throw new RateLimitError(`rate limited loading ${path}`);
  if (!res.ok) throw new Error(`content ${res.status} for ${path}`);
  return res.text();
}

// The fetched content a file's diff renders from. `patch` reconstructs the
// before side from `after` (one request); `full` carries both sides. Kept
// separate from rendering so a theme change re-renders without re-fetching.
export type Sides =
  | { kind: "patch"; after: string; patch: string; path: string }
  | { kind: "full"; before: string; after: string; path: string };

// Fetch the content one changed file's diff renders from, or `null` when the
// file is binary. For modified/renamed files that carry a `patch`, fetch only
// the head-side content and let the engine reconstruct the before side (one
// request instead of two). Otherwise resolve both sides. This is the network
// half of rendering; cache the result and re-run `renderSides` on theme change.
// `auth` is the GitHub token ("" for anonymous), passed in because the review
// worker cannot read `localStorage`.
export async function loadSides(
  repoRef: PrRef,
  file: PrFile,
  baseSha: string,
  headSha: string,
  auth: string,
): Promise<Sides | null> {
  const canPatch =
    (file.status === "modified" || file.status === "renamed") &&
    typeof file.patch === "string" &&
    file.patch.length > 0;

  if (canPatch) {
    const after = await fetchContent(repoRef, file.filename, headSha, auth);
    if (looksBinary(after)) return null;
    return { kind: "patch", after, patch: file.patch as string, path: file.filename };
  }

  const { before, after, path } = await resolveSides(
    repoRef,
    file,
    baseSha,
    headSha,
    auth,
  );
  if (looksBinary(before) || looksBinary(after)) return null;
  return { kind: "full", before, after, path };
}

export function githubSidesSource(
  repoRef: PrRef,
  files: PrFile[],
  baseSha: string,
  headSha: string,
  auth: () => string,
): { fetch(index: number): Promise<Sides | null>; remaining(): number | null } {
  return {
    fetch: (index) => loadSides(repoRef, files[index], baseSha, headSha, auth()),
    remaining: rateRemaining,
  };
}

// Render already-fetched `sides` to HTML with the given syntax `theme` (a
// registry name; "" selects the default). Pure and synchronous, so it can be
// re-run on a theme switch against cached `Sides`.
export function renderSides(engine: Engine, sides: Sides, theme: string): string {
  return sides.kind === "patch"
    ? engine.renderFromPatch(sides.after, sides.patch, sides.path, theme)
    : engine.renderFile(sides.before, sides.after, sides.path, theme);
}

// Resolve before/after content for a changed file based on its status.
async function resolveSides(
  repoRef: PrRef,
  file: PrFile,
  baseSha: string,
  headSha: string,
  auth: string,
): Promise<{ before: string; after: string; path: string }> {
  const newPath = file.filename;
  const oldPath = file.previous_filename || file.filename;
  switch (file.status) {
    case "added":
      return {
        before: "",
        after: await fetchContent(repoRef, newPath, headSha, auth),
        path: newPath,
      };
    case "removed":
      return {
        before: await fetchContent(repoRef, oldPath, baseSha, auth),
        after: "",
        path: oldPath,
      };
    default: {
      const [before, after] = await Promise.all([
        fetchContent(repoRef, oldPath, baseSha, auth),
        fetchContent(repoRef, newPath, headSha, auth),
      ]);
      return { before, after, path: newPath };
    }
  }
}

import { looksBinary, type PrRef } from "./lib";

// ---------------------------------------------------------------------------
// GitHub API client
// ---------------------------------------------------------------------------

// Signed in, requests go through the Worker, which adds the user's GitHub
// token (it never reaches the browser; the session cookie authenticates).
// Signed out, they go straight to GitHub anonymously (60/hr per IP).
function apiBase(viaWorker: boolean): string {
  return viaWorker ? "/api/github" : "https://api.github.com";
}

const SIGN_IN_EXPIRED = "Your sign-in expired. Reload the page or sign in again.";

const JSON_MEDIA = "application/vnd.github+json";
// The raw media type returns the file body directly and works up to 100 MB;
// the JSON form leaves `content` empty for files over 1 MB. GitHub gives both
// forms the same ETag, so a browser cache holding the JSON form revalidates
// it with a 304 and serves the JSON body for a raw request (seen in
// Firefox). Raw requests therefore bypass the HTTP cache.
const RAW_MEDIA = "application/vnd.github.raw+json";

interface GhError extends Error {
  detail?: string;
}

export class RateLimitError extends Error {}

let lastRemaining: number | null = null;

export function rateRemaining(): number | null {
  return lastRemaining;
}

// Forget the last seen budget, e.g. after a sign-in change, when it belongs to
// the other identity.
export function resetRateRemaining(): void {
  lastRemaining = null;
}

async function ghFetch(url: string, accept = JSON_MEDIA): Promise<Response> {
  const res = await fetch(url, {
    headers: { Accept: accept },
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

async function gh<T>(
  path: string,
  viaWorker: boolean,
): Promise<{ data: T; remaining: string | null }> {
  const res = await ghFetch(`${apiBase(viaWorker)}${path}`);
  const remaining = res.headers.get("x-ratelimit-remaining");
  if (!res.ok) {
    const body = await res.text();
    let message = `${res.status} ${res.statusText}`;
    if (isRateLimited(res)) {
      message = viaWorker
        ? "GitHub rate limit reached for your account."
        : "GitHub rate limit reached (60/hr). Sign in with GitHub for 5000/hr.";
    } else if (res.status === 404) {
      message = viaWorker
        ? "Not found. Private repo? Reauthorize GitHub from your account menu, or ask the repo's organization to approve the deltoids review app."
        : "Not found. Private repo? Sign in with GitHub.";
    } else if (res.status === 401) {
      message = SIGN_IN_EXPIRED;
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

export async function fetchPr(ref: PrRef, viaWorker: boolean): Promise<Pr> {
  const { data } = await gh<Pr>(
    `/repos/${ref.owner}/${ref.repo}/pulls/${ref.number}`,
    viaWorker,
  );
  return data;
}

// One open pull request from GitHub's issue search, enough for a list row.
export interface PrSummary {
  html_url: string;
  number: number;
  title: string;
  draft?: boolean;
  updated_at: string;
  user: { login: string } | null;
}

// The signed-in user's open pull requests matching `query` (search qualifiers
// such as `author:@me`), most recently updated first, at most 30. `@me` needs
// the user's token, so this always goes through the Worker.
export async function searchPrs(query: string): Promise<PrSummary[]> {
  const q = `is:pr is:open archived:false ${query}`;
  const { data } = await gh<{ items: PrSummary[] }>(
    `/search/issues?q=${encodeURIComponent(q)}&sort=updated&order=desc&per_page=30`,
    true,
  );
  return data.items;
}

export async function fetchFiles(ref: PrRef, viaWorker: boolean): Promise<PrFile[]> {
  const files: PrFile[] = [];
  for (let page = 1; page <= 30; page++) {
    const { data } = await gh<PrFile[]>(
      `/repos/${ref.owner}/${ref.repo}/pulls/${ref.number}/files?per_page=100&page=${page}`,
      viaWorker,
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
  viaWorker: boolean,
): Promise<string> {
  const url = `${apiBase(viaWorker)}/repos/${repoRef.owner}/${repoRef.repo}/contents/${encodeURIComponent(
    path,
  ).replace(/%2F/g, "/")}?ref=${encodeURIComponent(ref)}`;
  const res = await ghFetch(url, RAW_MEDIA);
  if (res.status === 404) return "";
  if (isRateLimited(res)) throw new RateLimitError(`rate limited loading ${path}`);
  if (res.status === 401) throw new Error(SIGN_IN_EXPIRED);
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
// half of rendering; the engine's `openShape` renders it in any theme.
// `viaWorker` routes requests through the Worker (signed in) or straight to
// GitHub (signed out).
export async function loadSides(
  repoRef: PrRef,
  file: PrFile,
  baseSha: string,
  headSha: string,
  viaWorker: boolean,
): Promise<Sides | null> {
  const canPatch =
    (file.status === "modified" || file.status === "renamed") &&
    typeof file.patch === "string" &&
    file.patch.length > 0;

  if (canPatch) {
    const after = await fetchContent(repoRef, file.filename, headSha, viaWorker);
    if (looksBinary(after)) return null;
    return { kind: "patch", after, patch: file.patch as string, path: file.filename };
  }

  const { before, after, path } = await resolveSides(
    repoRef,
    file,
    baseSha,
    headSha,
    viaWorker,
  );
  if (looksBinary(before) || looksBinary(after)) return null;
  return { kind: "full", before, after, path };
}

export function githubSidesSource(
  repoRef: PrRef,
  files: PrFile[],
  baseSha: string,
  headSha: string,
  viaWorker: () => boolean,
): { fetch(index: number): Promise<Sides | null>; remaining(): number | null } {
  return {
    fetch: (index) => loadSides(repoRef, files[index], baseSha, headSha, viaWorker()),
    remaining: rateRemaining,
  };
}

// Render already-fetched `sides` to HTML with the given syntax `theme` (a
// registry name; "" selects the default). Pure and synchronous, so it can be
// re-run on a theme switch against cached `Sides`.

// Resolve before/after content for a changed file based on its status.
async function resolveSides(
  repoRef: PrRef,
  file: PrFile,
  baseSha: string,
  headSha: string,
  viaWorker: boolean,
): Promise<{ before: string; after: string; path: string }> {
  const newPath = file.filename;
  const oldPath = file.previous_filename || file.filename;
  switch (file.status) {
    case "added":
      return {
        before: "",
        after: await fetchContent(repoRef, newPath, headSha, viaWorker),
        path: newPath,
      };
    case "removed":
      return {
        before: await fetchContent(repoRef, oldPath, baseSha, viaWorker),
        after: "",
        path: oldPath,
      };
    default: {
      const [before, after] = await Promise.all([
        fetchContent(repoRef, oldPath, baseSha, viaWorker),
        fetchContent(repoRef, newPath, headSha, viaWorker),
      ]);
      return { before, after, path: newPath };
    }
  }
}

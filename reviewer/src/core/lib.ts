// Pure, DOM-free helpers for the reviewer. Kept separate so they can be
// unit-tested without a browser environment.

export interface PrRef {
  owner: string;
  repo: string;
  number: number;
}

// Parse a GitHub PR reference from a full URL or `owner/repo/number`.
export function parsePrUrl(input: string | null | undefined): PrRef | null {
  const s = (input || "").trim();
  let m = s.match(/github\.com\/([^/]+)\/([^/]+)\/pull\/(\d+)/i);
  if (!m) m = s.match(/^([^/\s]+)\/([^/\s]+)\/(\d+)$/);
  if (!m) return null;
  return { owner: m[1], repo: m[2], number: Number(m[3]) };
}

// Heuristic: treat content with a NUL byte as binary.
export function looksBinary(text: string): boolean {
  return text.includes("\u0000");
}

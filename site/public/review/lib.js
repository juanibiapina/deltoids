// Pure, DOM-free helpers for the reviewer. Kept separate from app.js so they
// can be unit-tested under `node --test`.

// Parse a GitHub PR reference from a full URL or `owner/repo/number`.
export function parsePrUrl(input) {
  const s = (input || "").trim();
  let m = s.match(/github\.com\/([^/]+)\/([^/]+)\/pull\/(\d+)/i);
  if (!m) m = s.match(/^([^/\s]+)\/([^/\s]+)\/(\d+)$/);
  if (!m) return null;
  return { owner: m[1], repo: m[2], number: Number(m[3]) };
}

// Decode a base64 string (GitHub contents API `content`) into UTF-8 text.
export function decodeBase64Utf8(b64) {
  const binary = atob((b64 || "").replace(/\n/g, ""));
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
  return new TextDecoder().decode(bytes);
}

// Heuristic: treat content with a NUL byte as binary.
export function looksBinary(text) {
  return text.includes("\u0000");
}

// Map a GitHub file status to a badge CSS modifier.
export function badgeClass(status) {
  if (status === "added") return "added";
  if (status === "removed") return "removed";
  if (status === "renamed") return "renamed";
  return "";
}

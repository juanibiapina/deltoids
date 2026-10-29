// Read-only GitHub proxy for signed-in users: forwards the reviewer's four
// request shapes (PR, file list, file contents, the home page's PR search) with
// the user's token, which never leaves the Worker.

const PREFIX = "/api/github";
const ALLOWED =
  /^(\/repos\/[^/]+\/[^/]+\/(pulls\/\d+(\/files)?|contents\/.+)|\/search\/issues)$/;
const PASSED_HEADERS = [
  "content-type",
  "x-ratelimit-limit",
  "x-ratelimit-remaining",
  "x-ratelimit-reset",
];

export async function proxyGithub(request: Request, token: string): Promise<Response> {
  const url = new URL(request.url);
  const path = url.pathname.slice(PREFIX.length);
  if (!ALLOWED.test(path)) return Response.json({ error: "not found" }, { status: 404 });
  if (request.method !== "GET") {
    return Response.json({ error: "method not allowed" }, { status: 405 });
  }

  const upstream = await fetch(`https://api.github.com${path}${url.search}`, {
    headers: {
      Accept: request.headers.get("accept") ?? "application/vnd.github+json",
      Authorization: `Bearer ${token}`,
      // GitHub rejects requests without a User-Agent; a Worker fetch sends none.
      "User-Agent": "deltoids-review",
      "X-GitHub-Api-Version": "2022-11-28",
    },
  });

  const headers = new Headers({ "Cache-Control": "no-store" });
  for (const name of PASSED_HEADERS) {
    const value = upstream.headers.get(name);
    if (value !== null) headers.set(name, value);
  }
  return new Response(upstream.body, { status: upstream.status, headers });
}

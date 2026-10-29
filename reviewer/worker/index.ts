import { authenticate, githubToken } from "./auth";
import type { Env } from "./env";
import { proxyGithub } from "./github";

// Runs only for requests no static asset matched.
export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    const url = new URL(request.url);
    if (!url.pathname.startsWith("/api/")) return notFound();

    const userId = await authenticate(request, env);
    if (userId === null) return json({ error: "unauthenticated" }, 401);

    if (request.method === "GET" && url.pathname === "/api/me") {
      return json({ userId });
    }
    if (url.pathname.startsWith("/api/github/")) {
      const token = await githubToken(userId, env);
      if (!token) return json({ error: "no github token" }, 401);
      return proxyGithub(request, token);
    }
    return notFound();
  },
} satisfies ExportedHandler<Env>;

function json(body: unknown, status = 200): Response {
  return Response.json(body, { status });
}

function notFound(): Response {
  return json({ error: "not found" }, 404);
}

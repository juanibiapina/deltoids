import { authenticate } from "./auth";
import type { Env } from "./env";

// Static assets (reviewer/dist) are served before this runs, so the Worker
// only sees requests no file matched. Every `/api/*` route is authenticated
// here; a route receives the signed-in user id and never imports Clerk.
export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    const url = new URL(request.url);
    if (!url.pathname.startsWith("/api/")) return notFound();

    const userId = await authenticate(request, env);
    if (userId === null) return json({ error: "unauthenticated" }, 401);

    if (request.method === "GET" && url.pathname === "/api/me") {
      return json({ userId });
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

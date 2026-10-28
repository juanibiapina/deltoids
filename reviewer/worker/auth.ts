import { createClerkClient } from "@clerk/backend";
import type { Env } from "./env";

// The only server code that knows about Clerk. Returns the signed-in user's
// id, or null when the request carries no valid session (from the browser's
// `__session` cookie or an `Authorization: Bearer` header).
//
// Tokens are verified locally against `CLERK_JWT_KEY`, so production never
// fetches Clerk's JWKS and runs the same path the tests run. The page and the
// API share an origin, so the request's own origin is the only authorized
// party. Errors Clerk throws (a malformed key, a missing secret) propagate:
// the Worker answers 500 and the error lands in the Worker logs.
export async function authenticate(request: Request, env: Env): Promise<string | null> {
  const clerk = createClerkClient({
    secretKey: env.CLERK_SECRET_KEY,
    publishableKey: env.CLERK_PUBLISHABLE_KEY,
    jwtKey: env.CLERK_JWT_KEY,
    telemetry: { disabled: true },
  });
  const state = await clerk.authenticateRequest(request, {
    authorizedParties: [new URL(request.url).origin],
  });
  if (!state.isAuthenticated) return null;
  return state.toAuth().userId;
}

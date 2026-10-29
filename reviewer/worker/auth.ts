import { createClerkClient } from "@clerk/backend";
import type { Env } from "./env";

// jwtKey keeps verification local, so production runs the path the tests run.
function clerkClient(env: Env) {
  return createClerkClient({
    secretKey: env.CLERK_SECRET_KEY,
    publishableKey: env.CLERK_PUBLISHABLE_KEY,
    jwtKey: env.CLERK_JWT_KEY,
    telemetry: { disabled: true },
  });
}

// Page and API share an origin, so it is the only authorized party.
export async function authenticate(request: Request, env: Env): Promise<string | null> {
  const state = await clerkClient(env).authenticateRequest(request, {
    authorizedParties: [new URL(request.url).origin],
  });
  if (!state.isAuthenticated) return null;
  return state.toAuth().userId;
}

// How long a resolved GitHub token is reused before asking Clerk again. Bounds
// how long a reauthorized user (new scopes, new token) waits for the new one.
const TOKEN_TTL_MS = 30_000;

// Best effort, per isolate: Workers keep module state between requests but do
// not route a user to the same isolate, so a miss only costs one Clerk call.
// It holds resolved strings, never an in-flight promise: a cancelled request's
// promise could stay pending and hang every later request for that user.
const tokens = new Map<string, { token: string; expires: number }>();

// The GitHub OAuth access token Clerk holds for the user, or "" when they have
// none. Clerk errors propagate and cache nothing.
export async function githubToken(userId: string, env: Env): Promise<string> {
  const now = Date.now();
  const cached = tokens.get(userId);
  if (cached && cached.expires > now) return cached.token;
  const { data } = await clerkClient(env).users.getUserOauthAccessToken(userId, "github");
  const token = data[0]?.token ?? "";
  tokens.set(userId, { token, expires: now + TOKEN_TTL_MS });
  return token;
}

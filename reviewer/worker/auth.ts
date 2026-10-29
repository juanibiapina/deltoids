import { createClerkClient } from "@clerk/backend";
import type { Env } from "./env";

// jwtKey keeps verification local, so production runs the path the tests run.
// Page and API share an origin, so it is the only authorized party.
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

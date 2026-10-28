// @vitest-environment node
import { exportSPKI, generateKeyPair, SignJWT } from "jose";
import worker from "./index";
import type { Env } from "./env";

// Configured like production: a well-formed `pk_live_` key for the Frontend
// API host `clerk.example.com`, and the PEM public key that verifies tokens
// locally, so no case touches the network.
const FRONTEND_API = "clerk.example.com";
const ORIGIN = "https://review.example.com";
const USER_ID = "user_2abc";
// One fixed time for the token's `iat` and the `__client_uat` cookie: Clerk
// rejects a token issued before the client's last sign-in.
const NOW = Math.floor(Date.now() / 1000);

let env: Env;
let privateKey: CryptoKey;

beforeAll(async () => {
  const pair = await generateKeyPair("RS256", { extractable: true });
  privateKey = pair.privateKey;
  env = {
    CLERK_PUBLISHABLE_KEY: `pk_live_${btoa(`${FRONTEND_API}$`)}`,
    CLERK_SECRET_KEY: "sk_live_test",
    CLERK_JWT_KEY: await exportSPKI(pair.publicKey),
  };
});

function sessionToken({ azp = ORIGIN }: { azp?: string } = {}): Promise<string> {
  return new SignJWT({ azp, sid: "sess_1" })
    .setProtectedHeader({ alg: "RS256", kid: "ins_1" })
    .setIssuer(`https://${FRONTEND_API}`)
    .setSubject(USER_ID)
    .setIssuedAt(NOW)
    .setNotBefore(NOW - 10)
    .setExpirationTime(NOW + 60)
    .sign(privateKey);
}

function get(path: string, headers: Record<string, string> = {}): Promise<Response> {
  return worker.fetch(new Request(`${ORIGIN}${path}`, { headers }), env);
}

describe("GET /api/me", () => {
  test("signed out is 401", async () => {
    const res = await get("/api/me");
    expect(res.status).toBe(401);
  });

  test("a garbage bearer token is 401", async () => {
    const res = await get("/api/me", { authorization: "Bearer not-a-token" });
    expect(res.status).toBe(401);
  });

  test("a valid session token returns the user id", async () => {
    const res = await get("/api/me", { authorization: `Bearer ${await sessionToken()}` });
    expect(res.status).toBe(200);
    expect(await res.json()).toEqual({ userId: USER_ID });
  });

  test("a token issued for another origin is 401", async () => {
    const token = await sessionToken({ azp: "https://evil.example.com" });
    const res = await get("/api/me", { authorization: `Bearer ${token}` });
    expect(res.status).toBe(401);
  });

  test("the browser's session cookies return the user id", async () => {
    const res = await get("/api/me", {
      cookie: `__session=${await sessionToken()}; __client_uat=${NOW}`,
      "sec-fetch-dest": "empty",
    });
    expect(res.status).toBe(200);
    expect(await res.json()).toEqual({ userId: USER_ID });
  });
});

test("an unknown /api route is 404 when signed in", async () => {
  const res = await get("/api/unknown", { authorization: `Bearer ${await sessionToken()}` });
  expect(res.status).toBe(404);
});

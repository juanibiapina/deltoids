// @vitest-environment node
import { exportSPKI, generateKeyPair, SignJWT } from "jose";
import type { Env } from "./env";

// Clerk's client and the GitHub proxy both call the global fetch. Install a
// delegate before the Worker is imported, so each test sets `handler`.
const outbound = vi.hoisted(() => {
  const state = {
    handler: (_req: Request): Promise<Response> | Response =>
      new Response("unexpected request", { status: 599 }),
    requests: [] as Request[],
  };
  globalThis.fetch = async (input: RequestInfo | URL, init?: RequestInit) => {
    const req = new Request(input, init);
    state.requests.push(req);
    return state.handler(req);
  };
  return state;
});

import worker from "./index";

const FRONTEND_API = "clerk.example.com";
const ORIGIN = "https://review.example.com";
const USER_ID = "user_2abc";
// Shared by the token's iat and __client_uat: Clerk rejects a token issued
// before the client's last sign-in.
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

function sessionToken({
  azp = ORIGIN,
  sub = USER_ID,
}: { azp?: string; sub?: string } = {}): Promise<string> {
  return new SignJWT({ azp, sid: "sess_1" })
    .setProtectedHeader({ alg: "RS256", kid: "ins_1" })
    .setIssuer(`https://${FRONTEND_API}`)
    .setSubject(sub)
    .setIssuedAt(NOW)
    .setNotBefore(NOW - 10)
    .setExpirationTime(NOW + 60)
    .sign(privateKey);
}

function get(
  path: string,
  headers: Record<string, string> = {},
  method = "GET",
): Promise<Response> {
  return worker.fetch(new Request(`${ORIGIN}${path}`, { headers, method }), env);
}

beforeEach(() => {
  outbound.requests.length = 0;
  outbound.handler = () => new Response("unexpected request", { status: 599 });
});

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

describe("/api/github/*", () => {
  // Clerk's answer for a user's GitHub OAuth token, as its Backend API sends it.
  function clerkTokens(tokens: string[]): Response {
    return Response.json({
      data: tokens.map((token) => ({
        object: "oauth_access_token",
        external_account_id: "eac_1",
        token,
        provider: "oauth_github",
        public_metadata: {},
        label: null,
        scopes: ["read:user", "user:email"],
      })),
      total_count: tokens.length,
    });
  }

  // Answers Clerk with `token` and GitHub with a small JSON body.
  function stubUpstream(token = "gho_x") {
    outbound.handler = (req) => {
      const host = new URL(req.url).host;
      if (host === "api.clerk.com") return clerkTokens([token]);
      if (host === "api.github.com") {
        return new Response('{"number":1}', {
          status: 200,
          headers: {
            "content-type": "application/json; charset=utf-8",
            "x-ratelimit-limit": "5000",
            "x-ratelimit-remaining": "4999",
            "x-ratelimit-reset": "1700000000",
            etag: '"abc"',
          },
        });
      }
      return new Response("unexpected request", { status: 599 });
    };
  }

  const toHost = (host: string) =>
    outbound.requests.filter((r) => new URL(r.url).host === host);

  async function signedIn(sub: string) {
    return { authorization: `Bearer ${await sessionToken({ sub })}` };
  }

  test("signed out is 401 and reaches no upstream", async () => {
    const res = await get("/api/github/repos/o/r/pulls/1");
    expect(res.status).toBe(401);
    expect(outbound.requests).toHaveLength(0);
  });

  test("forwards a PR request with the user's token", async () => {
    stubUpstream("gho_x");
    const res = await get("/api/github/repos/o/r/pulls/1", {
      ...(await signedIn("user_forward")),
      accept: "application/vnd.github+json",
    });

    expect(res.status).toBe(200);
    expect(await res.text()).toBe('{"number":1}');
    expect(res.headers.get("x-ratelimit-remaining")).toBe("4999");
    expect(res.headers.get("cache-control")).toBe("no-store");
    expect(res.headers.get("etag")).toBeNull();

    const [gh] = toHost("api.github.com");
    expect(gh.url).toBe("https://api.github.com/repos/o/r/pulls/1");
    expect(gh.headers.get("authorization")).toBe("Bearer gho_x");
    expect(gh.headers.get("user-agent")).toBeTruthy();
    expect(gh.headers.get("accept")).toBe("application/vnd.github+json");
  });

  test("passes GitHub's error status through", async () => {
    outbound.handler = (req) =>
      new URL(req.url).host === "api.clerk.com"
        ? clerkTokens(["gho_x"])
        : new Response('{"message":"Not Found"}', { status: 404 });
    const res = await get("/api/github/repos/o/r/pulls/9", await signedIn("user_404"));
    expect(res.status).toBe(404);
  });

  test("forwards a contents request with its query", async () => {
    stubUpstream();
    const res = await get("/api/github/repos/o/r/contents/a/b.ts?ref=sha", {
      ...(await signedIn("user_contents")),
      accept: "application/vnd.github.raw+json",
    });
    expect(res.status).toBe(200);
    const [gh] = toHost("api.github.com");
    expect(gh.url).toBe("https://api.github.com/repos/o/r/contents/a/b.ts?ref=sha");
    expect(gh.headers.get("accept")).toBe("application/vnd.github.raw+json");
  });

  test("forwards a file-list request", async () => {
    stubUpstream();
    const res = await get(
      "/api/github/repos/o/r/pulls/1/files?per_page=100&page=2",
      await signedIn("user_files"),
    );
    expect(res.status).toBe(200);
    expect(toHost("api.github.com")[0].url).toBe(
      "https://api.github.com/repos/o/r/pulls/1/files?per_page=100&page=2",
    );
  });

  test("forwards a pull request search with its query", async () => {
    stubUpstream("gho_x");
    const res = await get(
      "/api/github/search/issues?q=is%3Apr%20author%3A%40me&per_page=30",
      await signedIn("user_search"),
    );
    expect(res.status).toBe(200);
    const [gh] = toHost("api.github.com");
    expect(gh.url).toBe("https://api.github.com/search/issues?q=is%3Apr%20author%3A%40me&per_page=30");
    expect(gh.headers.get("authorization")).toBe("Bearer gho_x");
  });

  test("another search path is 404", async () => {
    stubUpstream();
    const res = await get("/api/github/search/code?q=x", await signedIn("user_search_code"));
    expect(res.status).toBe(404);
    expect(toHost("api.github.com")).toHaveLength(0);
  });

  test("a path outside the allowlist is 404", async () => {
    stubUpstream();
    const res = await get("/api/github/user", await signedIn("user_allow"));
    expect(res.status).toBe(404);
    expect(toHost("api.github.com")).toHaveLength(0);
  });

  test("a POST is 405", async () => {
    stubUpstream();
    const res = await get("/api/github/repos/o/r/pulls/1", await signedIn("user_post"), "POST");
    expect(res.status).toBe(405);
    expect(toHost("api.github.com")).toHaveLength(0);
  });

  test("two requests in a row ask Clerk once", async () => {
    stubUpstream();
    const headers = await signedIn("user_cache");
    await get("/api/github/repos/o/r/pulls/1", headers);
    await get("/api/github/repos/o/r/pulls/1/files", headers);
    expect(toHost("api.clerk.com")).toHaveLength(1);
    expect(toHost("api.github.com")).toHaveLength(2);
  });

  test("a Clerk failure is not cached", async () => {
    outbound.handler = () =>
      Response.json({ errors: [{ code: "too_many_requests", message: "slow down" }] }, {
        status: 429,
      });
    const headers = await signedIn("user_clerk_fail");
    await expect(get("/api/github/repos/o/r/pulls/1", headers)).rejects.toThrow();

    stubUpstream();
    const res = await get("/api/github/repos/o/r/pulls/1", headers);
    expect(res.status).toBe(200);
    expect(toHost("api.clerk.com")).toHaveLength(2);
  });

  test("a user with no GitHub token is 401", async () => {
    outbound.handler = () => clerkTokens([]);
    const res = await get("/api/github/repos/o/r/pulls/1", await signedIn("user_none"));
    expect(res.status).toBe(401);
    expect(await res.json()).toEqual({ error: "no github token" });
    expect(toHost("api.github.com")).toHaveLength(0);
  });
});

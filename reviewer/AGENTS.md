# reviewer

Standalone React PR reviewer (Vite, React 18, TypeScript, Vitest with jsdom).
It runs as the Cloudflare Worker `deltoids-review` at `review.deltoids.dev`
and renders a GitHub pull request as a scope-expanded diff with the
`crates/deltoids-wasm` engine. The same Worker serves a small backend under
`/api`. Sign-in is optional: reviewing works signed out.

`src/core/` holds the DOM-free logic, `src/components/` and `src/hooks/` the
React UI, and `worker/` the backend. `public/deltoids_wasm.wasm` is the
engine, a gitignored build product.

## Dev / build / test

```bash
cd reviewer
npm install
# Build the engine once (needs wasi-sdk; see crates/deltoids-wasm/AGENTS.md):
DEST="$PWD/public/deltoids_wasm.wasm" \
  WASI_SDK=/path/to/wasi-sdk ../crates/deltoids-wasm/build-wasm.sh
npm run dev        # http://localhost:5173 (dev Clerk key from ZeroVault)
npm run dev:api    # build + Worker on http://localhost:8787, with sign-in
npm run build      # tsc --noEmit && vite build -> reviewer/dist/ (needs the key)
npm run preview    # serve dist (no Worker)
npm test           # vitest run (app + worker/)
npm run typecheck  # tsc for the app and for worker/
npx wrangler deploy --dry-run --outdir=../worker-dist  # bundle check, as CI
```

## Backend

The Worker (`wrangler.toml`) serves `dist/` as static assets and runs
`worker/index.ts` only when no file matches. Deep links are query strings on
`/` (`?pr=...`), so there is no single-page-app fallback. Page and API share
the origin `review.deltoids.dev`, so the Clerk `__session` cookie reaches
`/api/*` with no CORS.

- `worker/index.ts` authenticates every `/api/*` request with
  `auth.ts::authenticate`, answers `401` when signed out, then dispatches on
  the path. To add a route, add a branch to that dispatch. Routes get the
  user id and never import Clerk: `worker/auth.ts` is the only Clerk-aware
  server code.
- `/api/github/*` (`worker/github.ts`) forwards a signed-in user's GitHub
  reads with their token, which never reaches the browser. Only `GET` on an
  allowlist of paths passes (other paths 404, other methods 405), so the
  route is no general GitHub client. Responses carry
  `Cache-Control: no-store`, so a cached private response cannot outlive an
  account switch.
- `auth.ts::githubToken` caches each user's GitHub token in module memory
  for a short TTL. One review makes one proxied request per file, and the
  Clerk Backend API limits request rates. The cache never holds an
  in-flight promise (a cancelled request could hang later ones), caches
  nothing on a Clerk failure, and does not evict on a GitHub 401 (Clerk
  returns the same revoked token). KV was rejected because it stores the
  tokens of users in a second place.
- `authenticate` verifies tokens locally against `CLERK_JWT_KEY` with the
  request's own origin as the only authorized party. Production never
  fetches the Clerk JWKS, so it runs the path that the tests run.
- `vite build` fails without `VITE_CLERK_PUBLISHABLE_KEY`. Component tests
  mock `@clerk/react`.

### Clerk

One Clerk application, with GitHub as the only sign-in method:

- Production (`pk_live_`, Juan's GitHub OAuth app `deltoids review`) serves
  `review.deltoids.dev`. It asks GitHub for the `repo` scope (Clerk dashboard
  → SSO connections → GitHub → Scopes), because GitHub has no read-only
  scope for private repos. `AccountButton` passes
  `additionalOAuthScopes` only for a `pk_live_` key.
- Development (`pk_test_`, shared Clerk GitHub credentials) serves local
  dev, because a production instance rejects `localhost`. It cannot grant
  `repo` and reviews public repos only.

The Worker secrets are `CLERK_PUBLISHABLE_KEY`, `CLERK_SECRET_KEY`, and
`CLERK_JWT_KEY`. `[secrets] required` aborts a real deploy when one was never
set (`--dry-run` does not check).

### Local dev with sign-in

Use `npm run dev:api` and open `http://localhost:8787`. Rerun it for page
changes. Its `--local-upstream` flag keeps `request.url` on `localhost:8787`.
Without it, wrangler rewrites the URL to the production route and every
signed-in request gets 401. A Vite proxy from `:5173` fails the same check.
Under plain `npm run dev` there is no Worker, so signed-in reviews fail with
"Not found". Sign out there.

Both scripts need a ZeroVault sign-in for the `deltoids` organization:

1. Switch the active organization to `deltoids` at `dash.zeroapps.dev`.
2. From `reviewer/`, run `npx zero login --context deltoids` once per machine.
3. From `reviewer/`, run `npx zero context use deltoids` once per checkout.

Run `zero` only from `reviewer/`. The repo root has no `package.json`, so
`npx zero` there fetches an unrelated npm package.

### Secrets

Every credential lives in ZeroVault (organization `deltoids`), one project
per consumer, each with `development` and `production`:

| Project | Keys | Read by |
| --- | --- | --- |
| `deltoids-review` | `CLERK_PUBLISHABLE_KEY`, `CLERK_SECRET_KEY`, `CLERK_JWT_KEY` (both envs) | `wrangler dev` via `--mount .dev.vars`; production Worker secrets via `bin/sync-secrets` |
| `deltoids-review-web` | `VITE_CLERK_PUBLISHABLE_KEY` (both envs) | `npm run dev`, `dev:api`, and the CI build on `main` |
| `deltoids-ci` | `CLOUDFLARE_API_TOKEN`, `CLOUDFLARE_ACCOUNT_ID` (production) | the deploy step (OIDC) and `bin/sync-secrets` |

- Never write a real `.dev.vars` or `.env*.local`. `--mount` refuses to run
  when a real `.dev.vars` exists, and a stale one would win silently.
- The publishable key is stored twice per environment:
  `CLERK_PUBLISHABLE_KEY` in `deltoids-review/<env>` and
  `VITE_CLERK_PUBLISHABLE_KEY` in `deltoids-review-web/<env>`. Both copies
  must hold the same key. A mismatch makes every sign-in 401.
- `deltoids-ci` is separate so that the sync, which pushes a whole
  environment, never turns the Cloudflare token into a Worker secret.
- Run `bin/sync-secrets` by hand after any change to
  `deltoids-review/production` (first setup, key rotation). It copies that
  environment to the Worker. Until you run it after a `CLERK_JWT_KEY`
  rotation, every signed-in request gets 401.

## Deploy

`.github/workflows/reviewer.yml` builds the engine, tests, type-checks, and
bundles the Worker on every PR. PR runs cannot read ZeroVault, so they use a
placeholder key and never deploy. On `main`, the job trades its GitHub OIDC
token for a ZeroVault token and runs `wrangler deploy` with `deltoids-ci`.
The repo holds no Cloudflare secret, and CI does not sync Worker secrets.

## Invariants

- Signed-in GitHub requests go through `/api/github`. Signed-out requests go
  straight to `api.github.com`, so users do not share the Worker's one
  anonymous budget. Until Clerk has loaded, the deep link waits and Review is
  disabled, so a signed-in user's first request does not go out anonymously.
- `App` sets the review worker's `viaWorker` flag with `setWorkerSignedIn`
  before any `openReview`.
- `App` decides during render whether to show the home list, because a
  child's effects run before App's deep-link effect.
- A theme switch must not fetch from GitHub again. Fetching (`loadSides`) is
  separate from the pure `renderSides`, and `RenderSession` re-renders from
  the sides it holds.
- The `index.html` preload of `/deltoids_wasm.wasm` (`as="fetch"` +
  `crossorigin`) must match the `compileStreaming` request in
  `core/engine.ts`. Otherwise the engine downloads twice.
- The diff selection lives in `ReviewView` state. The tree library's
  controlled `expandedIds` was rejected: collapsing a directory also folds
  its descendants, out of sync with the prop.

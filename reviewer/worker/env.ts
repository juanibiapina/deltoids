// The Worker's bindings. `CLERK_PUBLISHABLE_KEY` is a plain var
// (wrangler.toml [vars]); the other two are Worker secrets synced from
// ZeroVault by bin/sync-secrets. All three are required.
export interface Env {
  CLERK_PUBLISHABLE_KEY: string;
  CLERK_SECRET_KEY: string;
  CLERK_JWT_KEY: string;
}

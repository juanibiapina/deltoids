import { createContext, type ReactNode, useContext } from "react";
import { ClerkProvider, Show, SignInButton, UserButton } from "@clerk/react";

// Sign-in is optional: with no publishable key (local `npm run dev`, forks,
// tests) the app runs exactly as before, with no account button. Anything
// rendered outside AuthProvider sees auth off, so components never touch Clerk
// without its provider around them.
const AuthEnabled = createContext(false);

export function AuthProvider({ children }: { children: ReactNode }) {
  // The only reader of the key. Production builds get the committed pk_live_
  // key from .env.production; `npm run dev:api` passes a pk_test_ key.
  const publishableKey = import.meta.env.VITE_CLERK_PUBLISHABLE_KEY as string | undefined;
  if (!publishableKey) return <>{children}</>;
  return (
    <ClerkProvider
      publishableKey={publishableKey}
      routerPush={followClerk}
      routerReplace={followClerk}
    >
      <AuthEnabled.Provider value={true}>{children}</AuthEnabled.Provider>
    </ClerkProvider>
  );
}

// Clerk routes its same-origin navigations here. The one that matters is
// sign-out, which goes to `afterSignOutUrl` ("/" by default) and would reload
// the start page, dropping the open review (`?pr=...`). Clerk reads that URL
// once at load and the default sign-out menu item cannot be replaced, so the
// rule lives here: a target without a query string keeps the current one, and
// a target equal to the current page needs no reload because Clerk's
// components follow the session on their own. Navigations to other origins
// (GitHub, Clerk's pages) never reach these hooks.
function followClerk(to: string): void {
  const destination = clerkDestination(to, window.location.href);
  if (destination !== null) window.location.assign(destination);
}

// Where a Clerk navigation from `current` to `to` should go, or null to stay.
export function clerkDestination(to: string, current: string): string | null {
  const url = new URL(to, current);
  const here = new URL(current);
  if (!url.search) url.search = here.search;
  return url.href === here.href ? null : url.href;
}

export function AccountButton() {
  if (!useContext(AuthEnabled)) return null;
  return (
    <div className="account">
      <Show when="signed-out">
        <SignInButton mode="modal">
          <button type="button" className="tool">
            Sign in
          </button>
        </SignInButton>
      </Show>
      <Show when="signed-in">
        <UserButton />
      </Show>
    </div>
  );
}

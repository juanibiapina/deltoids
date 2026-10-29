import { createContext, type ReactNode, useContext } from "react";
import { ClerkProvider, Show, SignInButton, UserButton } from "@clerk/react";

// A context instead of reading the key in each component: anything rendered
// outside AuthProvider (tests, even with a .env.local key) sees auth off.
const AuthEnabled = createContext(false);

export function AuthProvider({ children }: { children: ReactNode }) {
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

// Sign-out navigates to "/" and would drop the open review (?pr=...). Clerk
// reads afterSignOutUrl once at load and its sign-out item cannot be replaced,
// so its same-origin navigations keep the query string here.
function followClerk(to: string): void {
  const destination = clerkDestination(to, window.location.href);
  if (destination !== null) window.location.assign(destination);
}

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

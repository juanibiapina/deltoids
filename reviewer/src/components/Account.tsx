import type { ReactNode } from "react";
import {
  ClerkProvider,
  Show,
  SignInButton,
  UserButton,
  useAuth,
  useClerk,
} from "@clerk/react";

const PUBLISHABLE_KEY = import.meta.env.VITE_CLERK_PUBLISHABLE_KEY;

// Production asks GitHub for `repo` (GitHub has no read-only scope for private
// repos), so the account panel offers users who signed in before to
// reauthorize. Development uses Clerk's shared credentials, which cannot grant
// it. Tests run without a key.
const userProfileProps = (PUBLISHABLE_KEY ?? "").startsWith("pk_live_")
  ? { additionalOAuthScopes: { github: ["repo"] } }
  : undefined;

export function AuthProvider({ children }: { children: ReactNode }) {
  return (
    <ClerkProvider publishableKey={PUBLISHABLE_KEY}>
      {children}
    </ClerkProvider>
  );
}

export function AccountButton() {
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
        <UserButton userProfileProps={userProfileProps} />
      </Show>
    </div>
  );
}

// Whether GitHub requests may go out yet (`loaded`) and whether they go
// through the Worker (`signedIn`). When clerk-js fails to load (an ad blocker,
// Clerk down), `isLoaded` stays false for good, so that counts as loaded and
// signed out. `isLoaded` stays true across sign-in and sign-out.
export function useGitHubAccess(): { loaded: boolean; signedIn: boolean } {
  const { isLoaded, isSignedIn } = useAuth();
  const failed = useClerk().status === "error";
  return {
    loaded: isLoaded || failed,
    signedIn: !failed && Boolean(isSignedIn),
  };
}

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
    <ClerkProvider publishableKey={publishableKey}>
      <AuthEnabled.Provider value={true}>{children}</AuthEnabled.Provider>
    </ClerkProvider>
  );
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

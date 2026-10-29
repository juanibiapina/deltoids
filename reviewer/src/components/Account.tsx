import type { ReactNode } from "react";
import { ClerkProvider, Show, SignInButton, UserButton } from "@clerk/react";

export function AuthProvider({ children }: { children: ReactNode }) {
  return (
    <ClerkProvider publishableKey={import.meta.env.VITE_CLERK_PUBLISHABLE_KEY}>
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
        <UserButton />
      </Show>
    </div>
  );
}

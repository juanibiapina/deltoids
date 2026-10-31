import type { ReactNode } from "react";

// Stands in for @clerk/react in `vite --mode e2e`: Clerk is loaded and the
// user is signed out, so the reviewer reads GitHub directly, where the
// Playwright tests serve a recorded pull request. The top bar shows the same
// Sign in button as production; it opens nothing.
export function ClerkProvider({ children }: { children: ReactNode }) {
  return <>{children}</>;
}
export const Show = ({ when, children }: { when: string; children: ReactNode }) =>
  when === "signed-out" ? <>{children}</> : null;
export const SignInButton = ({ children }: { children: ReactNode }) => <>{children}</>;
export const UserButton = () => null;
export const useAuth = () => ({ isLoaded: true, isSignedIn: false });
export const useClerk = () => ({ status: "ready" });

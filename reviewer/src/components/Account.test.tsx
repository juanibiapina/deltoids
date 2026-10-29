import { renderHook } from "@testing-library/react";
import { expect, test, vi } from "vitest";
import { useGitHubAccess } from "./Account";

const clerk = vi.hoisted(() => ({
  auth: { isLoaded: false, isSignedIn: undefined as boolean | undefined },
  status: "loading",
}));

vi.mock("@clerk/react", () => ({
  ClerkProvider: () => null,
  Show: () => null,
  SignInButton: () => null,
  UserButton: () => null,
  useAuth: () => clerk.auth,
  useClerk: () => ({ status: clerk.status }),
}));

test("waits while Clerk loads", () => {
  clerk.auth = { isLoaded: false, isSignedIn: undefined };
  clerk.status = "loading";
  const { result } = renderHook(() => useGitHubAccess());
  expect(result.current).toEqual({ loaded: false, signedIn: false });
});

test("reports a loaded, signed-in session", () => {
  clerk.auth = { isLoaded: true, isSignedIn: true };
  clerk.status = "ready";
  const { result } = renderHook(() => useGitHubAccess());
  expect(result.current).toEqual({ loaded: true, signedIn: true });
});

test("counts a failed clerk-js as loaded and signed out", () => {
  clerk.auth = { isLoaded: false, isSignedIn: undefined };
  clerk.status = "error";
  const { result } = renderHook(() => useGitHubAccess());
  expect(result.current).toEqual({ loaded: true, signedIn: false });
});

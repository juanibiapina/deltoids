import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { render, waitFor } from "@testing-library/react";
import { App } from "./App";

const clerk = vi.hoisted(() => ({ signedIn: true }));

vi.mock("@clerk/react", () => ({
  Show: () => null,
  SignInButton: () => null,
  UserButton: () => null,
  useAuth: () => ({ isLoaded: true, isSignedIn: clerk.signedIn }),
  useClerk: () => ({ status: "ready" }),
}));

// The review worker needs `Worker`, which jsdom lacks.
vi.mock("./core/reviewClient", () => ({
  startReviewWorker: () => ({ ready: Promise.resolve() }),
  setWorkerSignedIn: () => {},
  openReview: () => new Promise(() => {}),
}));

let fetch: ReturnType<typeof vi.fn>;

beforeEach(() => {
  fetch = vi.fn(() => Promise.resolve(Response.json({ items: [] })));
  vi.stubGlobal("fetch", fetch);
  // jsdom lacks both; the top bar measures itself and queries the width.
  vi.stubGlobal(
    "ResizeObserver",
    class {
      observe() {}
      disconnect() {}
    },
  );
  vi.stubGlobal("matchMedia", (query: string) => ({
    matches: false,
    media: query,
    addEventListener: () => {},
    removeEventListener: () => {},
  }));
});

afterEach(() => {
  vi.unstubAllGlobals();
  history.replaceState(null, "", "/");
});

function searches(): string[] {
  return fetch.mock.calls
    .map(([url]) => String(url))
    .filter((url) => url.startsWith("/api/github/search/issues"));
}

test("signed in, home searches for the user's pull requests", async () => {
  clerk.signedIn = true;
  history.replaceState(null, "", "/");
  render(<App />);
  await waitFor(() => expect(searches()).toHaveLength(4));
});

test("signed in, a deep link opens the review without searching", async () => {
  clerk.signedIn = true;
  history.replaceState(null, "", "/?pr=o/r/1");
  render(<App />);
  await waitFor(() =>
    expect(fetch).toHaveBeenCalledWith("/api/github/repos/o/r/pulls/1", expect.anything()),
  );
  expect(searches()).toHaveLength(0);
});

test("signed out, home makes no search request", async () => {
  clerk.signedIn = false;
  history.replaceState(null, "", "/");
  render(<App />);
  await new Promise((resolve) => setTimeout(resolve, 0));
  expect(searches()).toHaveLength(0);
});

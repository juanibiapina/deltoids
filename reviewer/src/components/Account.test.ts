import { describe, expect, test } from "vitest";
import { clerkDestination } from "./Account";

const REVIEW = "https://review.deltoids.dev/?pr=owner%2Frepo%2F123";

describe("clerkDestination", () => {
  test("sign-out to the root keeps the open review in place", () => {
    expect(clerkDestination("/", REVIEW)).toBeNull();
  });

  test("a target without a query string keeps the current one", () => {
    expect(clerkDestination("/elsewhere", REVIEW)).toBe(
      "https://review.deltoids.dev/elsewhere?pr=owner%2Frepo%2F123",
    );
  });

  test("a target with its own query string is followed as is", () => {
    expect(clerkDestination("/?pr=a%2Fb%2F1", REVIEW)).toBe("https://review.deltoids.dev/?pr=a%2Fb%2F1");
  });
});

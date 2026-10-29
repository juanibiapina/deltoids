import { afterEach, expect, test, vi } from "vitest";
import { fireEvent, render, screen, within } from "@testing-library/react";
import { PullRequestList } from "./PullRequestList";

const pr = {
  html_url: "https://github.com/acme/widgets/pull/42",
  number: 42,
  title: "Fix the widget",
  draft: true,
  updated_at: new Date().toISOString(),
  user: { login: "ana" },
};

// Answers each section's search by its qualifier; a qualifier mapped to a
// number answers with that HTTP status.
function stubSearch(answers: Record<string, unknown[] | number>) {
  const fetch = vi.fn((url: string) => {
    const q = new URL(url, "https://review.deltoids.dev").searchParams.get("q") ?? "";
    const qualifier = q.split(" ").pop() ?? "";
    const answer = answers[qualifier] ?? [];
    return Promise.resolve(
      typeof answer === "number"
        ? new Response("{}", { status: answer })
        : Response.json({ items: answer }),
    );
  });
  vi.stubGlobal("fetch", fetch);
  return fetch;
}

afterEach(() => {
  vi.unstubAllGlobals();
});

test("lists each section's pull requests with repo, number, and title", async () => {
  stubSearch({ "review-requested:@me": [pr] });
  render(<PullRequestList onOpen={() => {}} />);
  const section = screen.getByRole("region", { name: "Review requests" });
  const row = await within(section).findByRole("link");
  expect(row.textContent).toContain("acme/widgets");
  expect(row.textContent).toContain("#42");
  expect(row.textContent).toContain("Fix the widget");
  expect(row.textContent).toContain("Draft");
  expect(row.getAttribute("href")).toBe(`?${new URLSearchParams({ pr: pr.html_url })}`);
});

test("a section with no pull requests says so", async () => {
  stubSearch({});
  render(<PullRequestList onOpen={() => {}} />);
  const section = screen.getByRole("region", { name: "Created" });
  expect(await within(section).findByText("You have no open pull requests.")).toBeTruthy();
});

test("a failed search shows the error in its section", async () => {
  stubSearch({ "mentions:@me": 401 });
  render(<PullRequestList onOpen={() => {}} />);
  const section = screen.getByRole("region", { name: "Mentioned" });
  expect(await within(section).findByText(/sign-in expired/)).toBeTruthy();
});

test("clicking a row opens the pull request", async () => {
  stubSearch({ "author:@me": [pr] });
  const onOpen = vi.fn();
  render(<PullRequestList onOpen={onOpen} />);
  const row = await screen.findByRole("link");
  expect(fireEvent.click(row)).toBe(false);
  expect(onOpen).toHaveBeenCalledWith(pr.html_url);
});

test.each([{ metaKey: true }, { ctrlKey: true }])(
  "a modified click (%o) keeps the link's default",
  async (modifier) => {
    stubSearch({ "author:@me": [pr] });
    const onOpen = vi.fn();
    render(<PullRequestList onOpen={onOpen} />);
    const row = await screen.findByRole("link");
    // fireEvent.click returns false when the default was prevented.
    expect(fireEvent.click(row, modifier)).toBe(true);
    expect(onOpen).not.toHaveBeenCalled();
  },
);

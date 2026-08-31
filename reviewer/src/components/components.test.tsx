import { describe, expect, test, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { Sidebar } from "./Sidebar";
import type { PrFile } from "../core/github";

describe("Sidebar", () => {
  const files: PrFile[] = [
    { filename: "src/a.ts", status: "modified" },
    { filename: "src/b.ts", status: "added" },
    { filename: "old.ts", status: "removed" },
  ];

  test("renders one anchor per file linking to its card", () => {
    render(<Sidebar files={files} onNavigate={() => {}} />);
    const links = screen.getAllByRole("link");
    expect(links).toHaveLength(3);
    expect(links[0].getAttribute("href")).toBe("#file-0");
    expect(links[1].getAttribute("href")).toBe("#file-1");
    expect(links[1].className).toContain("added");
    expect(links[2].className).toContain("removed");
  });

  test("clicking a file fires onNavigate (closes the drawer)", () => {
    const onNavigate = vi.fn();
    render(<Sidebar files={files} onNavigate={onNavigate} />);
    fireEvent.click(screen.getAllByRole("link")[0]);
    expect(onNavigate).toHaveBeenCalledOnce();
  });
});

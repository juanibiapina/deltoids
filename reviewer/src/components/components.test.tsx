import { createRef } from "react";
import { describe, expect, test, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { FileTree } from "./FileTree";
import { Topbar } from "./Topbar";
import type { Prefs } from "../hooks/usePrefs";
import type { PrFile } from "../core/github";

const files: PrFile[] = [
  { filename: "src/a.ts", status: "modified" },
  { filename: "src/b.ts", status: "added" },
  { filename: "README.md", status: "modified" },
];

describe("FileTree", () => {
  test("renders directory headers and file leaves, expanded by default", () => {
    render(<FileTree files={files} onFileSelect={() => {}} />);
    // Grouped directory header.
    expect(screen.getByText("src")).toBeTruthy();
    // Leaves show basenames (visible because dirs default-expanded).
    expect(screen.getByText("a.ts")).toBeTruthy();
    expect(screen.getByText("b.ts")).toBeTruthy();
    expect(screen.getByText("README.md")).toBeTruthy();
    // The container is a WAI-ARIA tree.
    expect(screen.getByRole("tree")).toBeTruthy();
  });

  test("selecting a file leaf reports its index", () => {
    const onFileSelect = vi.fn();
    render(<FileTree files={files} onFileSelect={onFileSelect} />);
    fireEvent.click(screen.getByText("a.ts"));
    expect(onFileSelect).toHaveBeenCalledWith(0);
  });

  test("collapsing a directory hides its files", () => {
    render(<FileTree files={files} onFileSelect={() => {}} />);
    expect(screen.getByText("a.ts")).toBeTruthy();
    fireEvent.click(screen.getByText("src"));
    expect(screen.queryByText("a.ts")).toBeNull();
    // A top-level file stays visible.
    expect(screen.getByText("README.md")).toBeTruthy();
  });
});

function makePrefs(overrides: Partial<Prefs> = {}): Prefs {
  return {
    nowrap: false,
    hideLineNumbers: true,
    size: "m",
    sizeIndex: 1,
    theme: "dark",
    syntaxTheme: "TokyoNight",
    syntaxThemeChoice: null,
    toggleWrap: () => {},
    toggleLineNumbers: () => {},
    stepSize: () => {},
    toggleTheme: () => {},
    setSyntaxTheme: () => {},
    ...overrides,
  };
}

function renderTopbar(prefs: Prefs) {
  return render(
    <Topbar
      topbarRef={createRef<HTMLElement>()}
      input=""
      onInput={() => {}}
      onSubmit={() => {}}
      hasToken={false}
      onToken={() => {}}
      showToolbar
      prefs={prefs}
      onFilesToggle={() => {}}
      drawerOpen={false}
    />,
  );
}

describe("Topbar theme toggle", () => {
  test("clicking the theme button calls toggleTheme", () => {
    const toggleTheme = vi.fn();
    renderTopbar(makePrefs({ theme: "dark", toggleTheme }));
    fireEvent.click(screen.getByTitle("Switch to light theme"));
    expect(toggleTheme).toHaveBeenCalledTimes(1);
  });

  test("reflects the current theme in title and aria-pressed", () => {
    renderTopbar(makePrefs({ theme: "light" }));
    const btn = screen.getByTitle("Switch to dark theme");
    expect(btn.getAttribute("aria-pressed")).toBe("true");
  });
});

describe("Topbar syntax-theme selector", () => {
  test("choosing a theme calls setSyntaxTheme with its name", () => {
    const setSyntaxTheme = vi.fn();
    renderTopbar(makePrefs({ setSyntaxTheme }));
    const select = screen.getByLabelText("Syntax theme") as HTMLSelectElement;
    fireEvent.change(select, { target: { value: "Dracula" } });
    expect(setSyntaxTheme).toHaveBeenCalledWith("Dracula");
  });

  test("choosing Auto calls setSyntaxTheme with null", () => {
    const setSyntaxTheme = vi.fn();
    renderTopbar(makePrefs({ syntaxThemeChoice: "Dracula", setSyntaxTheme }));
    const select = screen.getByLabelText("Syntax theme") as HTMLSelectElement;
    fireEvent.change(select, { target: { value: "" } });
    expect(setSyntaxTheme).toHaveBeenCalledWith(null);
  });
});

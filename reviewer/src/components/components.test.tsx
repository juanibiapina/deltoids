import { createRef } from "react";
import { afterEach, describe, expect, test, vi } from "vitest";
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

function renderTopbar(prefs: Prefs, overrides: Partial<Parameters<typeof Topbar>[0]> = {}) {
  return render(
    <Topbar
      topbarRef={createRef<HTMLElement>()}
      input=""
      onInput={() => {}}
      onSubmit={() => {}}
      hasToken={false}
      onToken={() => {}}
      started
      prefs={prefs}
      onFilesToggle={() => {}}
      drawerOpen={false}
      {...overrides}
    />,
  );
}

// Force useMediaQuery to report a given width class.
function mockWidth(wide: boolean) {
  window.matchMedia = vi.fn().mockImplementation((query: string) => ({
    matches: wide,
    media: query,
    onchange: null,
    addEventListener: () => {},
    removeEventListener: () => {},
    addListener: () => {},
    removeListener: () => {},
    dispatchEvent: () => false,
  })) as unknown as typeof window.matchMedia;
}

afterEach(() => {
  // @ts-expect-error reset the stub so absence (wide default) resumes.
  delete window.matchMedia;
});

function openSettings() {
  fireEvent.click(screen.getByTitle("Display settings"));
}

describe("Topbar controls — narrow (popover)", () => {
  test("opens on click and reports open state", () => {
    mockWidth(false);
    const onSettingsOpenChange = vi.fn();
    renderTopbar(makePrefs(), { onSettingsOpenChange });
    expect(screen.queryByRole("dialog")).toBeNull();
    openSettings();
    expect(screen.getByRole("dialog")).toBeTruthy();
    expect(onSettingsOpenChange).toHaveBeenCalledWith(true);
  });

  test("theme toggle inside the popover calls toggleTheme", () => {
    mockWidth(false);
    const toggleTheme = vi.fn();
    renderTopbar(makePrefs({ theme: "dark", toggleTheme }));
    openSettings();
    fireEvent.click(screen.getByTitle("Switch to light theme"));
    expect(toggleTheme).toHaveBeenCalledTimes(1);
  });
});

describe("Topbar controls — wide (inline)", () => {
  test("shows controls inline without a popover", () => {
    mockWidth(true);
    renderTopbar(makePrefs());
    expect(screen.queryByTitle("Display settings")).toBeNull();
    expect(screen.getByLabelText("Syntax theme")).toBeTruthy();
  });

  test("syntax-theme select calls setSyntaxTheme", () => {
    mockWidth(true);
    const setSyntaxTheme = vi.fn();
    renderTopbar(makePrefs({ setSyntaxTheme }));
    const select = screen.getByLabelText("Syntax theme") as HTMLSelectElement;
    fireEvent.change(select, { target: { value: "Dracula" } });
    expect(setSyntaxTheme).toHaveBeenCalledWith("Dracula");
  });
});

describe("Topbar PR input collapse", () => {
  test("narrow: hides the URL field after load and restores it on demand", () => {
    mockWidth(false);
    renderTopbar(makePrefs());
    expect(screen.queryByPlaceholderText(/github.com/)).toBeNull();
    fireEvent.click(screen.getByTitle("Load a different PR"));
    expect(screen.getByPlaceholderText(/github.com/)).toBeTruthy();
  });

  test("wide: keeps the URL field visible after load", () => {
    mockWidth(true);
    renderTopbar(makePrefs());
    expect(screen.getByPlaceholderText(/github.com/)).toBeTruthy();
  });

  test("narrow: keeps the URL field visible before a PR loads", () => {
    mockWidth(false);
    renderTopbar(makePrefs(), { started: false });
    expect(screen.getByPlaceholderText(/github.com/)).toBeTruthy();
  });
});

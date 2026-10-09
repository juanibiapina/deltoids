import { createRef } from "react";
import { afterEach, describe, expect, test, vi, type Mock } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { FileTree } from "./FileTree";
import { Topbar } from "./Topbar";
import { FileCard } from "./FileCard";
import { ReviewView, type ReviewData } from "./ReviewView";
import { LazyObserverProvider } from "./LazyObserver";
import * as github from "../core/github";
import type { Engine } from "../core/engine";
import type { Prefs } from "../hooks/usePrefs";
import type { PrFile } from "../core/github";

vi.mock("../core/github", async (orig) => {
  const actual = await orig<typeof import("../core/github")>();
  return { ...actual, loadSides: vi.fn(), renderSides: vi.fn() };
});

const files: PrFile[] = [
  { filename: "src/a.ts", status: "modified" },
  { filename: "src/b.ts", status: "added" },
  { filename: "README.md", status: "modified" },
];

describe("FileTree", () => {
  test("renders directory headers and file leaves, expanded by default", () => {
    render(<FileTree files={files} onSelect={() => {}} />);
    // Grouped directory header.
    expect(screen.getByText("src")).toBeTruthy();
    // Leaves show basenames (visible because dirs default-expanded).
    expect(screen.getByText("a.ts")).toBeTruthy();
    expect(screen.getByText("b.ts")).toBeTruthy();
    expect(screen.getByText("README.md")).toBeTruthy();
    // The container is a WAI-ARIA tree.
    expect(screen.getByRole("tree")).toBeTruthy();
  });

  test("selecting a file leaf reports a file selection", () => {
    const onSelect = vi.fn();
    render(<FileTree files={files} onSelect={onSelect} />);
    fireEvent.click(screen.getByText("a.ts"));
    expect(onSelect).toHaveBeenCalledWith({ kind: "file", index: 0 });
  });

  test("clicking a directory row selects it and keeps it open", () => {
    const onSelect = vi.fn();
    render(<FileTree files={files} onSelect={onSelect} />);
    fireEvent.click(screen.getByText("src"));
    expect(onSelect).toHaveBeenCalledWith({ kind: "dir", id: "src" });
    expect(screen.getByText("a.ts")).toBeTruthy();
  });

  test("the chevron folds a directory without selecting it", () => {
    const onSelect = vi.fn();
    render(<FileTree files={files} onSelect={onSelect} />);
    fireEvent.click(screen.getByTitle("Collapse"));
    expect(screen.queryByText("a.ts")).toBeNull();
    expect(screen.getByText("README.md")).toBeTruthy();
    expect(onSelect).not.toHaveBeenCalled();
  });

  test("a reviewed file row is dimmed and shows a check", () => {
    render(
      <FileTree
        files={files}
        onSelect={() => {}}
        isReviewed={(index) => index === 0}
      />,
    );
    const row = screen.getByText("a.ts").closest(".tree-file");
    expect(row?.classList.contains("reviewed")).toBe(true);
    expect(screen.getByTitle("Reviewed").textContent).toBe("✓");
    // An unreviewed file keeps its status letter, no reviewed class.
    const other = screen.getByText("b.ts").closest(".tree-file");
    expect(other?.classList.contains("reviewed")).toBe(false);
  });

  test("the selected file row is highlighted, others are not", () => {
    render(
      <FileTree files={files} onSelect={() => {}} selection={{ kind: "file", index: 1 }} />,
    );
    expect(document.querySelectorAll(".tree-row.active")).toHaveLength(1);
    const active = screen.getByText("b.ts").closest(".tree-file");
    expect(active?.classList.contains("active")).toBe(true);
  });

  test("the selected directory row is highlighted", () => {
    render(
      <FileTree files={files} onSelect={() => {}} selection={{ kind: "dir", id: "src" }} />,
    );
    expect(document.querySelectorAll(".tree-row.active")).toHaveLength(1);
    expect(screen.getByText("src").closest(".tree-dir")?.classList.contains("active")).toBe(true);
  });

  test("no row is highlighted without a selection", () => {
    render(<FileTree files={files} onSelect={() => {}} />);
    expect(document.querySelectorAll(".tree-row.active").length).toBe(0);
  });

  test("hideReviewed drops reviewed files from the tree", () => {
    render(
      <FileTree
        files={files}
        onSelect={() => {}}
        isReviewed={(index) => index === 0}
        hideReviewed
      />,
    );
    // src/a.ts (index 0) is reviewed → gone; its sibling b.ts stays.
    expect(screen.queryByText("a.ts")).toBeNull();
    expect(screen.getByText("b.ts")).toBeTruthy();
  });

  test("pruning the selected file does not crash the tree", () => {
    const { rerender } = render(
      <FileTree
        files={files}
        onSelect={() => {}}
        isReviewed={() => false}
        hideReviewed
      />,
    );
    // Select a.ts, then mark it reviewed so it is pruned from the tree. Without
    // remounting, react-accessible-treeview dereferences the removed selected
    // node id and throws, unmounting the app.
    fireEvent.click(screen.getByText("a.ts"));
    rerender(
      <FileTree
        files={files}
        onSelect={() => {}}
        isReviewed={(index) => index === 0}
        hideReviewed
      />,
    );
    expect(screen.queryByText("a.ts")).toBeNull();
    expect(screen.getByText("b.ts")).toBeTruthy();
  });
});

describe("FileCard gap expansion", () => {
  const AFTER = "l1\nl2\nl3\nl4\nl5\n";
  const GAP_HTML =
    '<div class="hunk"><div class="lineno">1</div></div>' +
    '<div class="gap" data-gap-lines="2" data-gap-new-start="2" ' +
    'data-gap-new-end="3"><span class="gap-label">2 unmodified lines</span></div>' +
    '<div class="hunk"><div class="lineno">4</div></div>';

  function mockEngine(): Engine {
    return {
      renderFile: vi.fn(),
      renderFromPatch: vi.fn(),
      renderContext: vi
        .fn()
        .mockReturnValue(
          '<div class="row context"><span class="ln">2</span>' +
            '<span class="code">l2</span></div>' +
            '<div class="row context"><span class="ln">3</span>' +
            '<span class="code">l3</span></div>',
        ),
    };
  }

  function renderCard(engine: Engine, syntaxTheme = "TokyoNight") {
    (github.loadSides as Mock).mockResolvedValue({
      kind: "full",
      before: "",
      after: AFTER,
      path: "x.rs",
    });
    // The real engine inlines theme colours, so its HTML differs per theme;
    // tag the output so a theme switch changes the string and React rebuilds
    // the `.gap` nodes (identical strings would skip the innerHTML reset).
    (github.renderSides as Mock).mockImplementation(
      (_engine: Engine, _sides: unknown, theme: string) =>
        `${GAP_HTML}<!--${theme}-->`,
    );
    return render(
      <LazyObserverProvider>
        <FileCard
          index={0}
          file={{ filename: "x.rs", status: "modified" }}
          engine={engine}
          repoRef={{ owner: "o", repo: "r", number: 1 }}
          baseSha="base"
          headSha="head"
          syntaxTheme={syntaxTheme}
          reviewed={false}
          onToggleReviewed={() => {}}
        />
      </LazyObserverProvider>,
    );
  }

  test("clicking a gap reveals its lines via renderContext", async () => {
    const engine = mockEngine();
    renderCard(engine);
    const gap = await screen.findByText("2 unmodified lines");
    fireEvent.click(gap);
    expect(engine.renderContext).toHaveBeenCalledWith(
      AFTER,
      "x.rs",
      2,
      3,
      "TokyoNight",
    );
    // The revealed context rows are injected and the divider is gone.
    expect(screen.getByText("l2")).toBeTruthy();
    expect(screen.getByText("l3")).toBeTruthy();
    expect(screen.queryByText("2 unmodified lines")).toBeNull();
    // The hunk that followed the gap is joined, folding its header away.
    expect(document.querySelectorAll(".hunk.joined").length).toBe(1);
  });

  test("a theme change re-applies the expansion with the new theme", async () => {
    const engine = mockEngine();
    const { rerender } = renderCard(engine);
    fireEvent.click(await screen.findByText("2 unmodified lines"));
    (engine.renderContext as Mock).mockClear();
    // Re-render with a different syntax theme; the card re-renders the diff
    // (new `.gap` nodes) and must re-expand with the new theme.
    rerender(
      <LazyObserverProvider>
        <FileCard
          index={0}
          file={{ filename: "x.rs", status: "modified" }}
          engine={engine}
          repoRef={{ owner: "o", repo: "r", number: 1 }}
          baseSha="base"
          headSha="head"
          syntaxTheme="GitHub"
          reviewed={false}
          onToggleReviewed={() => {}}
        />
      </LazyObserverProvider>,
    );
    expect(engine.renderContext).toHaveBeenCalledWith(
      AFTER,
      "x.rs",
      2,
      3,
      "GitHub",
    );
  });
});

describe("ReviewView selection", () => {
  function renderReview() {
    (github.loadSides as Mock).mockReturnValue(new Promise(() => {}));
    const data = {
      ref: { owner: "o", repo: "r", number: 1 },
      pr: { number: 1, title: "t", additions: 1, deletions: 1 },
      files,
      engine: {} as Engine,
      baseSha: "base",
      headSha: "head",
    } as unknown as ReviewData;
    return render(
      <ReviewView data={data} syntaxTheme="TokyoNight" hideViewed onNavigate={() => {}} />,
    );
  }

  const shownCards = () =>
    Array.from(document.querySelectorAll<HTMLElement>("section.file"))
      .filter((el) => !el.hidden)
      .map((el) => el.id);

  test("opens on the first file in tree order, alone", () => {
    renderReview();
    expect(shownCards()).toEqual(["file-2"]);
  });

  test("a file click shows only that file, a directory click shows its files", () => {
    renderReview();
    fireEvent.click(screen.getByText("b.ts"));
    expect(shownCards()).toEqual(["file-1"]);
    fireEvent.click(screen.getByText("src"));
    expect(shownCards()).toEqual(["file-0", "file-1"]);
  });
});

function makePrefs(overrides: Partial<Prefs> = {}): Prefs {
  return {
    nowrap: false,
    hideLineNumbers: true,
    hideViewed: true,
    size: "m",
    sizeIndex: 1,
    theme: "dark",
    syntaxTheme: "TokyoNight",
    syntaxThemeChoice: null,
    toggleWrap: () => {},
    toggleLineNumbers: () => {},
    toggleHideViewed: () => {},
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
  test("opens on click", () => {
    mockWidth(false);
    renderTopbar(makePrefs());
    expect(screen.queryByRole("dialog")).toBeNull();
    openSettings();
    expect(screen.getByRole("dialog")).toBeTruthy();
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

  test("Viewed toggle calls toggleHideViewed", () => {
    mockWidth(true);
    const toggleHideViewed = vi.fn();
    renderTopbar(makePrefs({ toggleHideViewed }));
    fireEvent.click(screen.getByTitle("Show files you've marked viewed"));
    expect(toggleHideViewed).toHaveBeenCalledTimes(1);
  });

  test("Viewed is pressed when viewed files are shown, not when hidden", () => {
    mockWidth(true);
    const { unmount } = renderTopbar(makePrefs({ hideViewed: true }));
    expect(
      screen.getByTitle("Show files you've marked viewed").getAttribute("aria-pressed"),
    ).toBe("false");
    unmount();
    renderTopbar(makePrefs({ hideViewed: false }));
    expect(
      screen.getByTitle("Show files you've marked viewed").getAttribute("aria-pressed"),
    ).toBe("true");
  });

  test("Line # is pressed when row numbers are shown, not when hidden", () => {
    mockWidth(true);
    const { unmount } = renderTopbar(makePrefs({ hideLineNumbers: true }));
    expect(
      screen.getByTitle("Show line numbers on diff rows").getAttribute("aria-pressed"),
    ).toBe("false");
    unmount();
    renderTopbar(makePrefs({ hideLineNumbers: false }));
    expect(
      screen.getByTitle("Show line numbers on diff rows").getAttribute("aria-pressed"),
    ).toBe("true");
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

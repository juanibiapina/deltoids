import { createRef } from "react";
import { afterEach, describe, expect, test, vi } from "vitest";
import { act, render, screen, fireEvent, waitFor } from "@testing-library/react";
import { FileTree } from "./FileTree";
import { Topbar } from "./Topbar";
import { FileCard } from "./FileCard";
import { ReviewView, type ReviewData } from "./ReviewView";
import { LazyObserverProvider } from "./LazyObserver";
import type { Prefs } from "../hooks/usePrefs";
import type { PrFile } from "../core/github";
import type { RenderSession, Rendered } from "../core/renderSession";

function fakeSession(overrides: Partial<RenderSession> = {}): RenderSession {
  return {
    render: vi.fn((_index: number, theme: string) =>
      Promise.resolve<Rendered>({ kind: "html", html: `<div class="row">${theme}</div>` }),
    ),
    renderContext: vi.fn(() => Promise.resolve("")),
    prioritize: vi.fn(),
    dispose: vi.fn(),
    ...overrides,
  };
}

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

  test("file rows lead with the change letter and end with line counts", () => {
    render(
      <FileTree
        files={[
          { filename: "src/a.ts", status: "modified", additions: 12, deletions: 3 },
          { filename: "src/b.ts", status: "added", additions: 5, deletions: 0 },
        ]}
        onSelect={() => {}}
      />,
    );
    const a = screen.getByText("a.ts").closest(".tree-file");
    expect(a?.textContent).toBe("Ma.ts+12-3");
    const b = screen.getByText("b.ts").closest(".tree-file");
    expect(b?.textContent).toBe("Ab.ts+5");
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
  const GAP_HTML =
    '<div class="hunk"><div class="lineno">1</div></div>' +
    '<div class="gap" data-gap-lines="2" data-gap-new-start="2" ' +
    'data-gap-new-end="3"><span class="gap-label">2 unmodified lines</span></div>' +
    '<div class="hunk"><div class="lineno">4</div></div>';
  const ROWS =
    '<div class="row context"><span class="ln">2</span>' +
    '<span class="code">l2</span></div>' +
    '<div class="row context"><span class="ln">3</span>' +
    '<span class="code">l3</span></div>';

  // The real engine inlines theme colours, so its HTML differs per theme; tag
  // the output so a theme switch changes the string and React rebuilds the
  // `.gap` nodes (identical strings would skip the innerHTML reset).
  const gapSession = () =>
    fakeSession({
      render: vi.fn((_index: number, theme: string) =>
        Promise.resolve<Rendered>({ kind: "html", html: `${GAP_HTML}<!--${theme}-->` }),
      ),
      renderContext: vi.fn(() => Promise.resolve(ROWS)),
    });

  function card(session: RenderSession, syntaxTheme = "TokyoNight") {
    return (
      <LazyObserverProvider>
        <FileCard
          index={0}
          file={{ filename: "x.rs", status: "modified" }}
          session={session}
          syntaxTheme={syntaxTheme}
          reviewed={false}
          onToggleReviewed={() => {}}
        />
      </LazyObserverProvider>
    );
  }

  test("clicking a gap reveals its lines via renderContext", async () => {
    const session = gapSession();
    render(card(session));
    fireEvent.click(await screen.findByText("2 unmodified lines"));
    // The revealed context rows are injected and the divider is gone.
    expect(await screen.findByText("l2")).toBeTruthy();
    expect(screen.getByText("l3")).toBeTruthy();
    expect(session.renderContext).toHaveBeenCalledWith(0, 2, 3, "TokyoNight");
    expect(screen.queryByText("2 unmodified lines")).toBeNull();
    // The hunk that followed the gap is joined, folding its header away.
    expect(document.querySelectorAll(".hunk.joined").length).toBe(1);
  });

  test("a theme change re-applies the expansion with the new theme", async () => {
    const session = gapSession();
    const { rerender } = render(card(session));
    fireEvent.click(await screen.findByText("2 unmodified lines"));
    await screen.findByText("l2");
    rerender(card(session, "GitHub"));
    await waitFor(() =>
      expect(session.renderContext).toHaveBeenCalledWith(0, 2, 3, "GitHub"),
    );
    expect(await screen.findByText("l2")).toBeTruthy();
  });
});

describe("FileCard rendering", () => {
  function card(session: RenderSession, syntaxTheme: string, hidden: boolean) {
    return (
      <LazyObserverProvider>
        <FileCard
          index={0}
          file={{ filename: "x.rs", status: "modified" }}
          session={session}
          syntaxTheme={syntaxTheme}
          reviewed={false}
          onToggleReviewed={() => {}}
          hidden={hidden}
        />
      </LazyObserverProvider>
    );
  }

  test("a hidden card renders in the background, a shown one urgently", async () => {
    const session = fakeSession();
    const { rerender } = render(card(session, "TokyoNight", true));
    await screen.findByText("TokyoNight");
    expect(session.render).toHaveBeenLastCalledWith(0, "TokyoNight", "background");

    rerender(card(session, "GitHub", true));
    await screen.findByText("GitHub");
    expect(session.render).toHaveBeenLastCalledWith(0, "GitHub", "background");

    rerender(card(session, "Nord", false));
    await screen.findByText("Nord");
    expect(session.render).toHaveBeenLastCalledWith(0, "Nord", "now");
  });

  test("a result for a theme no longer selected is dropped", async () => {
    let finishOld: (r: Rendered) => void = () => {};
    const session = fakeSession({
      render: vi.fn((_index: number, theme: string) =>
        theme === "TokyoNight"
          ? new Promise<Rendered>((resolve) => (finishOld = resolve))
          : Promise.resolve<Rendered>({ kind: "html", html: `<div class="row">${theme}</div>` }),
      ),
    });
    const { rerender } = render(card(session, "TokyoNight", false));
    rerender(card(session, "GitHub", false));
    await screen.findByText("GitHub");
    finishOld({ kind: "html", html: '<div class="row">TokyoNight</div>' });
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(screen.queryByText("TokyoNight")).toBeNull();
  });

  test("binary files and load failures show a notice", async () => {
    const binary = fakeSession({ render: vi.fn(() => Promise.resolve<Rendered>({ kind: "binary" })) });
    render(card(binary, "TokyoNight", false));
    expect(await screen.findByText("Binary file not shown.")).toBeTruthy();

    const failing = fakeSession({ render: vi.fn(() => Promise.reject(new Error("boom"))) });
    render(card(failing, "TokyoNight", false));
    expect(await screen.findByText("Could not load: boom")).toBeTruthy();
  });
});

describe("ReviewView selection", () => {
  function renderReview() {
    const data = {
      ref: { owner: "o", repo: "r", number: 1 },
      pr: { number: 1, title: "t", additions: 1, deletions: 1 },
      files,
      session: fakeSession({ render: vi.fn(() => new Promise<Rendered>(() => {})) }),
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

  function wheelPane(init: WheelEventInit) {
    const event = new WheelEvent("wheel", { cancelable: true, ...init });
    act(() => {
      document.querySelector(".pane")!.dispatchEvent(event);
    });
    return event;
  }

  test("ctrl+wheel over the diff steps through the sidebar rows", () => {
    renderReview();
    expect(shownCards()).toEqual(["file-2"]);
    const event = wheelPane({ deltaY: 100, ctrlKey: true });
    expect(event.defaultPrevented).toBe(true);
    expect(shownCards()).toEqual(["file-0", "file-1"]);
    wheelPane({ deltaY: -100, ctrlKey: true });
    expect(shownCards()).toEqual(["file-2"]);
  });

  test("shift+horizontal wheel steps too, and plain wheel does not", () => {
    renderReview();
    expect(wheelPane({ deltaY: 100 }).defaultPrevented).toBe(false);
    expect(shownCards()).toEqual(["file-2"]);
    wheelPane({ deltaX: 100, shiftKey: true });
    expect(shownCards()).toEqual(["file-0", "file-1"]);
  });

  test("the wheel skips rows inside a folded directory", () => {
    renderReview();
    fireEvent.click(screen.getByTitle("Collapse"));
    wheelPane({ deltaY: 100, ctrlKey: true });
    expect(shownCards()).toEqual(["file-0", "file-1"]);
    wheelPane({ deltaY: 100, ctrlKey: true });
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

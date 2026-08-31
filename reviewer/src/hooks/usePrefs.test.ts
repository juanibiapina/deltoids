import { beforeEach, describe, expect, test, vi } from "vitest";
import { act, renderHook } from "@testing-library/react";
import { SYNTAX_THEME_KEY, usePrefs } from "./usePrefs";

// Stub `matchMedia` (jsdom does not implement it) so the initial chrome theme
// is deterministic per test.
function mockColorScheme(prefersLight: boolean) {
  window.matchMedia = vi.fn().mockImplementation((query: string) => ({
    matches: query.includes("light") ? prefersLight : false,
    media: query,
    onchange: null,
    addListener: () => {},
    removeListener: () => {},
    addEventListener: () => {},
    removeEventListener: () => {},
    dispatchEvent: () => false,
  }));
}

describe("usePrefs syntax theme", () => {
  beforeEach(() => {
    localStorage.clear();
  });

  test("derives Tokyo Night on dark when unset", () => {
    mockColorScheme(false);
    const { result } = renderHook(() => usePrefs());
    expect(result.current.theme).toBe("dark");
    expect(result.current.syntaxThemeChoice).toBeNull();
    expect(result.current.syntaxTheme).toBe("TokyoNight");
  });

  test("derives GitHub on light when unset", () => {
    mockColorScheme(true);
    const { result } = renderHook(() => usePrefs());
    expect(result.current.theme).toBe("light");
    expect(result.current.syntaxTheme).toBe("GitHub");
  });

  test("explicit choice wins and persists", () => {
    mockColorScheme(false);
    const { result } = renderHook(() => usePrefs());
    act(() => result.current.setSyntaxTheme("Dracula"));
    expect(result.current.syntaxTheme).toBe("Dracula");
    expect(result.current.syntaxThemeChoice).toBe("Dracula");
    expect(localStorage.getItem(SYNTAX_THEME_KEY)).toBe("Dracula");
  });

  test("null reverts to the mode-derived default and clears storage", () => {
    mockColorScheme(false);
    localStorage.setItem(SYNTAX_THEME_KEY, "Dracula");
    const { result } = renderHook(() => usePrefs());
    expect(result.current.syntaxTheme).toBe("Dracula");
    act(() => result.current.setSyntaxTheme(null));
    expect(result.current.syntaxThemeChoice).toBeNull();
    expect(result.current.syntaxTheme).toBe("TokyoNight");
    expect(localStorage.getItem(SYNTAX_THEME_KEY)).toBeNull();
  });

  test("an unset choice follows a chrome theme toggle", () => {
    mockColorScheme(false);
    const { result } = renderHook(() => usePrefs());
    expect(result.current.syntaxTheme).toBe("TokyoNight");
    act(() => result.current.toggleTheme());
    expect(result.current.theme).toBe("light");
    expect(result.current.syntaxTheme).toBe("GitHub");
  });
});

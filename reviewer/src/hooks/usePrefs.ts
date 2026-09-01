import { useCallback, useMemo, useState } from "react";

import {
  DEFAULT_DARK_SYNTAX_THEME,
  DEFAULT_LIGHT_SYNTAX_THEME,
} from "../core/themes";

// Reading preferences (wrap + text size), persisted per browser under the same
// localStorage keys the original reviewer used.

const WRAP_KEY = "deltoids.review.nowrap";
const HIDE_LN_KEY = "deltoids.review.hide-ln";
const SIZE_KEY = "deltoids.review.size";
export const THEME_KEY = "deltoids.review.theme";
export const SYNTAX_THEME_KEY = "deltoids.review.syntax-theme";
export const SIZES = ["s", "m", "l"] as const;
export type Size = (typeof SIZES)[number];
export type Theme = "dark" | "light";

export interface Prefs {
  nowrap: boolean;
  hideLineNumbers: boolean;
  size: Size;
  sizeIndex: number;
  theme: Theme;
  // Resolved syntax-theme name passed to the wasm engine.
  syntaxTheme: string;
  // The user's explicit choice, or `null` when it derives from `theme`.
  syntaxThemeChoice: string | null;
  toggleWrap: () => void;
  toggleLineNumbers: () => void;
  stepSize: (delta: number) => void;
  toggleTheme: () => void;
  // Set an explicit syntax theme, or `null` to revert to the mode-derived
  // default. Persists across sessions.
  setSyntaxTheme: (name: string | null) => void;
}

// The mode-derived default syntax theme, used when the user has not chosen one.
function defaultSyntaxTheme(theme: Theme): string {
  return theme === "dark"
    ? DEFAULT_DARK_SYNTAX_THEME
    : DEFAULT_LIGHT_SYNTAX_THEME;
}

function initialSyntaxThemeChoice(): string | null {
  return localStorage.getItem(SYNTAX_THEME_KEY);
}

function initialSizeIndex(): number {
  const stored = (localStorage.getItem(SIZE_KEY) as Size) || "m";
  return Math.max(0, SIZES.indexOf(stored));
}

// Stored choice wins; otherwise follow the OS. Kept in sync with the inline
// pre-paint guard in index.html (same key, same fallback).
function initialTheme(): Theme {
  const stored = localStorage.getItem(THEME_KEY);
  if (stored === "light" || stored === "dark") return stored;
  return window.matchMedia?.("(prefers-color-scheme: light)").matches
    ? "light"
    : "dark";
}

export function usePrefs(): Prefs {
  const [nowrap, setNowrap] = useState(
    () => localStorage.getItem(WRAP_KEY) === "1",
  );
  // Line numbers are hidden by default; only an explicit "0" shows them.
  const [hideLineNumbers, setHideLineNumbers] = useState(
    () => localStorage.getItem(HIDE_LN_KEY) !== "0",
  );
  const [sizeIndex, setSizeIndex] = useState(initialSizeIndex);
  const [theme, setTheme] = useState<Theme>(initialTheme);
  const [syntaxThemeChoice, setSyntaxThemeChoice] = useState<string | null>(
    initialSyntaxThemeChoice,
  );

  const toggleWrap = useCallback(() => {
    setNowrap((prev) => {
      const next = !prev;
      localStorage.setItem(WRAP_KEY, next ? "1" : "0");
      return next;
    });
  }, []);

  const toggleLineNumbers = useCallback(() => {
    setHideLineNumbers((prev) => {
      const next = !prev;
      localStorage.setItem(HIDE_LN_KEY, next ? "1" : "0");
      return next;
    });
  }, []);

  const stepSize = useCallback((delta: number) => {
    setSizeIndex((prev) => {
      const next = Math.min(SIZES.length - 1, Math.max(0, prev + delta));
      localStorage.setItem(SIZE_KEY, SIZES[next]);
      return next;
    });
  }, []);

  const toggleTheme = useCallback(() => {
    setTheme((prev) => {
      const next = prev === "dark" ? "light" : "dark";
      localStorage.setItem(THEME_KEY, next);
      return next;
    });
  }, []);

  const setSyntaxTheme = useCallback((name: string | null) => {
    if (name === null) {
      localStorage.removeItem(SYNTAX_THEME_KEY);
    } else {
      localStorage.setItem(SYNTAX_THEME_KEY, name);
    }
    setSyntaxThemeChoice(name);
  }, []);

  // The explicit choice wins; otherwise derive from the chrome mode so a fresh
  // visitor gets Tokyo Night on dark, GitHub on light.
  const syntaxTheme = useMemo(
    () => syntaxThemeChoice ?? defaultSyntaxTheme(theme),
    [syntaxThemeChoice, theme],
  );

  return {
    nowrap,
    hideLineNumbers,
    size: SIZES[sizeIndex],
    sizeIndex,
    theme,
    syntaxTheme,
    syntaxThemeChoice,
    toggleWrap,
    toggleLineNumbers,
    stepSize,
    toggleTheme,
    setSyntaxTheme,
  };
}

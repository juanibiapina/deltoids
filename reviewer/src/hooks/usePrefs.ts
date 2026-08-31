import { useCallback, useState } from "react";

// Reading preferences (wrap + text size), persisted per browser under the same
// localStorage keys the original reviewer used.

const WRAP_KEY = "deltoids.review.nowrap";
const SIZE_KEY = "deltoids.review.size";
export const THEME_KEY = "deltoids.review.theme";
export const SIZES = ["s", "m", "l"] as const;
export type Size = (typeof SIZES)[number];
export type Theme = "dark" | "light";

export interface Prefs {
  nowrap: boolean;
  size: Size;
  sizeIndex: number;
  theme: Theme;
  toggleWrap: () => void;
  stepSize: (delta: number) => void;
  toggleTheme: () => void;
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
  const [sizeIndex, setSizeIndex] = useState(initialSizeIndex);
  const [theme, setTheme] = useState<Theme>(initialTheme);

  const toggleWrap = useCallback(() => {
    setNowrap((prev) => {
      const next = !prev;
      localStorage.setItem(WRAP_KEY, next ? "1" : "0");
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

  return {
    nowrap,
    size: SIZES[sizeIndex],
    sizeIndex,
    theme,
    toggleWrap,
    stepSize,
    toggleTheme,
  };
}

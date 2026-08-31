import { useCallback, useState } from "react";

// Reading preferences (wrap + text size), persisted per browser under the same
// localStorage keys the original reviewer used.

const WRAP_KEY = "deltoids.review.nowrap";
const SIZE_KEY = "deltoids.review.size";
export const SIZES = ["s", "m", "l"] as const;
export type Size = (typeof SIZES)[number];

export interface Prefs {
  nowrap: boolean;
  size: Size;
  sizeIndex: number;
  toggleWrap: () => void;
  stepSize: (delta: number) => void;
}

function initialSizeIndex(): number {
  const stored = (localStorage.getItem(SIZE_KEY) as Size) || "m";
  return Math.max(0, SIZES.indexOf(stored));
}

export function usePrefs(): Prefs {
  const [nowrap, setNowrap] = useState(
    () => localStorage.getItem(WRAP_KEY) === "1",
  );
  const [sizeIndex, setSizeIndex] = useState(initialSizeIndex);

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

  return {
    nowrap,
    size: SIZES[sizeIndex],
    sizeIndex,
    toggleWrap,
    stepSize,
  };
}

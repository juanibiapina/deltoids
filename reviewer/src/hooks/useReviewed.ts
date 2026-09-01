import { useCallback, useMemo, useState } from "react";

import type { PrFile } from "../core/github";
import type { PrRef } from "../core/lib";

// Per-file "reviewed" (a.k.a. "viewed") state, persisted per PR in
// localStorage. We store a map of `{ filename: blobSha }`: a file counts as
// reviewed only while the stored sha still matches the file's current blob sha
// at the PR head. Because the sha is content-addressed, a new commit that
// touches the file changes its sha, so the file auto-unmarks — exactly like
// GitHub/Bitbucket "Viewed" reset, at file granularity. Files that did not
// change keep their mark.

export interface Reviewed {
  /** True when `file` is marked reviewed at its current content. */
  isReviewed: (file: PrFile) => boolean;
  /** Flip `file`'s reviewed mark and persist. */
  toggle: (file: PrFile) => void;
  /** How many of the PR's files are currently reviewed. */
  count: number;
  /** Clear every mark for this PR. */
  clear: () => void;
}

type ShaMap = Record<string, string>;

function storageKey(ref: PrRef): string {
  return `deltoids.review.viewed:${ref.owner}/${ref.repo}/${ref.number}`;
}

// Read the persisted map. A missing or corrupt value yields an empty map so a
// bad entry can never throw on mount.
function load(key: string): ShaMap {
  try {
    const raw = localStorage.getItem(key);
    if (!raw) return {};
    const parsed = JSON.parse(raw);
    return parsed && typeof parsed === "object" ? (parsed as ShaMap) : {};
  } catch {
    return {};
  }
}

function save(key: string, map: ShaMap): void {
  if (Object.keys(map).length === 0) {
    localStorage.removeItem(key);
  } else {
    localStorage.setItem(key, JSON.stringify(map));
  }
}

export function useReviewed(ref: PrRef, files: PrFile[]): Reviewed {
  const key = storageKey(ref);
  // Re-seed the map when the target PR changes (the key is part of the state).
  const [state, setState] = useState<{ key: string; map: ShaMap }>(() => ({
    key,
    map: load(key),
  }));
  const map = state.key === key ? state.map : load(key);
  if (state.key !== key) {
    // Switched PRs mid-mount; adopt the new PR's persisted map.
    setState({ key, map });
  }

  const isReviewed = useCallback(
    (file: PrFile) => file.sha !== undefined && map[file.filename] === file.sha,
    [map],
  );

  const toggle = useCallback(
    (file: PrFile) => {
      const sha = file.sha;
      if (sha === undefined) return;
      setState((prev) => {
        const next = { ...prev.map };
        if (next[file.filename] === sha) {
          delete next[file.filename];
        } else {
          next[file.filename] = sha;
        }
        save(key, next);
        return { key, map: next };
      });
    },
    [key],
  );

  const clear = useCallback(() => {
    save(key, {});
    setState({ key, map: {} });
  }, [key]);

  const count = useMemo(
    () => files.reduce((n, f) => n + (isReviewed(f) ? 1 : 0), 0),
    [files, isReviewed],
  );

  return { isReviewed, toggle, count, clear };
}

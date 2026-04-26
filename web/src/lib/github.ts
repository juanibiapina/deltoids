/**
 * Fetch the deltoids star count from the GitHub API. Revalidates hourly
 * via the Next.js fetch cache so the value is fresh without hammering
 * the API on every request. Returns null on any failure (rate-limit,
 * network error, schema drift, etc.) so callers can fall back gracefully.
 */
export async function getDeltoidsStars(): Promise<number | null> {
  try {
    const res = await fetch(
      "https://api.github.com/repos/juanibiapina/deltoids",
      {
        headers: { Accept: "application/vnd.github+json" },
        next: { revalidate: 3600 },
      },
    );
    if (!res.ok) return null;
    const data = (await res.json()) as { stargazers_count?: unknown };
    return typeof data.stargazers_count === "number"
      ? data.stargazers_count
      : null;
  } catch {
    return null;
  }
}

/**
 * Compact star-count formatter: 0–999 stay as-is, 1000+ render as "1.2k",
 * 1_000_000+ as "1.2m". Matches GitHub's own header style.
 */
export function formatStars(n: number): string {
  if (n < 1_000) return String(n);
  if (n < 1_000_000) {
    const v = n / 1_000;
    return `${v >= 10 ? Math.round(v) : v.toFixed(1)}k`;
  }
  const v = n / 1_000_000;
  return `${v >= 10 ? Math.round(v) : v.toFixed(1)}m`;
}

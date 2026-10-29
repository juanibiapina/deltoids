import { type MouseEvent, useEffect, useState } from "react";
import { searchPrs, type PrSummary } from "../core/github";
import { parsePrUrl } from "../core/lib";

// The sections of GitHub's /pulls dashboard, one search each.
const SECTIONS = [
  { title: "Created", query: "author:@me", empty: "You have no open pull requests." },
  { title: "Assigned", query: "assignee:@me", empty: "No open pull requests are assigned to you." },
  { title: "Mentioned", query: "mentions:@me", empty: "No open pull requests mention you." },
  {
    title: "Review requests",
    query: "review-requested:@me",
    empty: "No open pull requests are waiting for your review.",
  },
];

type Result = { items: PrSummary[] } | { error: string };

interface PullRequestListProps {
  onOpen: (url: string) => void;
}

// The signed-in user's open pull requests on the home page. Each section loads
// once on mount; a click opens the review, a modified click the deep link.
export function PullRequestList({ onOpen }: PullRequestListProps) {
  const [results, setResults] = useState<(Result | null)[]>(() => SECTIONS.map(() => null));

  useEffect(() => {
    // StrictMode mounts twice in development; drop the first mount's answers.
    let cancelled = false;
    SECTIONS.forEach((section, i) => {
      searchPrs(section.query).then(
        (items) => ({ items }),
        (err: Error) => ({ error: err.message }),
      ).then((result) => {
        if (cancelled) return;
        setResults((prev) => prev.map((r, j) => (j === i ? result : r)));
      });
    });
    return () => {
      cancelled = true;
    };
  }, []);

  return (
    <div className="pr-list">
      {SECTIONS.map((section, i) => {
        const result = results[i];
        return (
          <section className="pr-section" key={section.title} aria-label={section.title}>
            <h2 className="pr-section-title">{section.title}</h2>
            <SectionBody result={result} empty={section.empty} onOpen={onOpen} />
          </section>
        );
      })}
    </div>
  );
}

function SectionBody({
  result,
  empty,
  onOpen,
}: {
  result: Result | null;
  empty: string;
  onOpen: (url: string) => void;
}) {
  if (!result) return <p className="pr-section-note">Loading…</p>;
  if ("error" in result) return <p className="pr-section-note error">{result.error}</p>;
  if (result.items.length === 0) return <p className="pr-section-note">{empty}</p>;
  return (
    <ul className="pr-rows">
      {result.items.map((pr) => (
        <li key={pr.html_url}>
          <PrRow pr={pr} onOpen={onOpen} />
        </li>
      ))}
    </ul>
  );
}

function PrRow({ pr, onOpen }: { pr: PrSummary; onOpen: (url: string) => void }) {
  const ref = parsePrUrl(pr.html_url);
  const onClick = (e: MouseEvent<HTMLAnchorElement>) => {
    // Middle and modified clicks keep the link's default: a new tab or window.
    if (e.button !== 0 || e.metaKey || e.ctrlKey || e.shiftKey || e.altKey) return;
    e.preventDefault();
    onOpen(pr.html_url);
  };
  return (
    <a
      className="pr-row"
      href={`?${new URLSearchParams({ pr: pr.html_url })}`}
      onClick={onClick}
    >
      <span className="pr-row-title">
        {pr.title}
        {pr.draft && <span className="pr-draft">Draft</span>}
      </span>
      <span className="pr-row-meta">
        {ref && (
          <span className="pr-repo">
            {ref.owner}/{ref.repo}
          </span>
        )}
        <span>#{pr.number}</span>
        {pr.user && <span>{pr.user.login}</span>}
        <time dateTime={pr.updated_at} title={new Date(pr.updated_at).toLocaleString()}>
          updated {relativeTime(pr.updated_at)}
        </time>
      </span>
    </a>
  );
}

const UNITS: [Intl.RelativeTimeFormatUnit, number][] = [
  ["year", 365 * 24 * 3600],
  ["month", 30 * 24 * 3600],
  ["week", 7 * 24 * 3600],
  ["day", 24 * 3600],
  ["hour", 3600],
  ["minute", 60],
];

function relativeTime(iso: string): string {
  const seconds = (Date.parse(iso) - Date.now()) / 1000;
  const format = new Intl.RelativeTimeFormat(undefined, { numeric: "auto" });
  for (const [unit, size] of UNITS) {
    if (Math.abs(seconds) >= size) return format.format(Math.round(seconds / size), unit);
  }
  return "just now";
}

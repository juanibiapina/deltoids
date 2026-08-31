// First-run empty state: explain the tool, offer example PRs, hint at tokens.

const EXAMPLES = [
  { label: "octocat/Spoon-Knife #41130", pr: "octocat/Spoon-Knife/41130" },
  { label: "earendil-works/pi #542", pr: "earendil-works/pi/542" },
];

export function Intro({ onExample }: { onExample: (pr: string) => void }) {
  return (
    <div className="intro">
      <h1>Review a GitHub pull request as a clean, scoped diff.</h1>
      <p>
        Paste a public PR URL above. deltoids renders each changed file with
        tree-sitter scope context — right here in your browser, no server.
      </p>
      <div className="examples">
        {EXAMPLES.map(({ label, pr }) => (
          <button
            key={pr}
            type="button"
            className="example"
            onClick={() => onExample(pr)}
          >
            {label}
          </button>
        ))}
      </div>
      <p className="hint">
        Hitting GitHub's rate limit? Add a read-only token with the 🔑 button
        (kept only in this browser). Deep-link any PR with <code>?pr=…</code>.
      </p>
    </div>
  );
}

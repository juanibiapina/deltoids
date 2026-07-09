/**
 * Single source of truth. Update here, do not hardcode.
 */

export const SITE = {
  name: "deltoids",
  domain: "deltoids.dev",
  url: "https://deltoids.dev",
  version: "0.12.0",
  license: "MIT",
  tagline: "Diffs for the agentic era.",
  description:
    "deltoids is a smart diff toolkit: a pager and terminal TUI that expand hunks to the enclosing scope, plus edit tools that trace every change your coding agent makes.",
  statusNote:
    "Beta: diff output may still be broken. Verify important changes.",
  repo: {
    owner: "juanibiapina",
    name: "deltoids",
    url: "https://github.com/juanibiapina/deltoids",
  },
  brewTap: "juanibiapina/taps",
  author: {
    name: "Juan Ibiapina",
    url: "https://github.com/juanibiapina",
  },
} as const;

export const NAV_LINKS = [
  { href: "/#pager", label: "Pager" },
  { href: "/#review", label: "Review" },
  { href: "/#install", label: "Install" },
  { href: "/#faq", label: "FAQ" },
  { href: "/docs/", label: "Docs" },
] as const;

export type PagerSnippet = {
  role: string;
  language: "bash" | "yaml" | "toml";
  code: string;
};

export type PagerTool = {
  id: string;
  name: string;
  snippets: PagerSnippet[];
};

/** `deltoids` integration snippets, grouped by tool. */
export const PAGER_TOOLS: PagerTool[] = [
  {
    id: "git",
    name: "git",
    snippets: [
      {
        role: "one-time",
        language: "bash",
        code: `git diff | deltoids | less -R`,
      },
      {
        role: "default pager",
        language: "bash",
        code: `git config --global core.pager 'deltoids | less -R'`,
      },
    ],
  },
  {
    id: "gh",
    name: "gh",
    snippets: [
      {
        role: "one-time",
        language: "bash",
        code: `gh pr diff <number> | deltoids | less -R`,
      },
      {
        role: "default pager",
        language: "bash",
        code: `gh config set pager 'deltoids | less -R'`,
      },
    ],
  },
  {
    id: "lazygit",
    name: "lazygit",
    snippets: [
      {
        role: "config",
        language: "yaml",
        code: `# ~/.config/lazygit/config.yml
git:
  paging:
    pager: deltoids`,
      },
    ],
  },
];

/** Agent tool install snippets per coding agent. */
export const AGENT_TOOLS: {
  id: string;
  label: string;
  code?: string;
  coming?: boolean;
  note?: string;
}[] = [
  {
    id: "pi",
    label: "pi",
    code: `pi install https://github.com/juanibiapina/deltoids`,
  },
  {
    id: "claude",
    label: "Claude Code",
    code: `claude plugin marketplace add juanibiapina/deltoids
claude plugin install deltoids@deltoids`,
    note: "Edits are recorded without per-edit summaries. Claude's PostToolUse hook does not expose one.",
  },
];

export type InstallCard = {
  id: string;
  label: string;
  /** Multi-line block, rendered as a single <pre><code>. */
  code: string;
};

/**
 * Verified install paths:
 * - brew formulas live at github.com/juanibiapina/homebrew-taps
 * - shell installer URL pattern is what cargo-dist emits per release
 * - cargo install --git is the documented from-source path in README
 */
export const INSTALL_CARDS: InstallCard[] = [
  {
    id: "homebrew",
    label: "homebrew",
    code: `brew install juanibiapina/taps/deltoids`,
  },
  {
    id: "shell",
    label: "shell installer",
    code: "curl -sSL https://github.com/juanibiapina/deltoids/releases/latest/download/deltoids-cli-installer.sh | sh",
  },
  {
    id: "cargo",
    label: "cargo (from source)",
    code: `cargo install --git https://github.com/juanibiapina/deltoids deltoids-cli`,
  },
];

/** FAQ. */
export const FAQ: { q: string; a: string }[] = [
  {
    q: "Is `deltoids` just a pager?",
    a: "No. Piped a diff, it's a pager. Run in a terminal, `deltoids` opens a TUI that browses your working tree and your coding agent's edits. It also ships `edit`, `write`, `hashread`, and `hashedit` tools for agents.",
  },
  {
    q: "How is this different from `git diff -W`?",
    a: "`git diff -W` finds scope with regex. `deltoids` parses the file with tree-sitter.",
  },
  {
    q: "Which languages get syntax highlighting?",
    a: "Any syntax bundled with syntect (e.g. Dockerfile), independent of tree-sitter scope support. Scope expansion covers the tree-sitter languages.",
  },
  {
    q: "Are diffs guaranteed correct?",
    a: "No. `deltoids` is beta; diff output may still be broken. Verify important changes with `git diff`.",
  },
];

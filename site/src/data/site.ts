/**
 * Single source of truth. Update here, do not hardcode.
 */

export const SITE = {
  name: "deltoids",
  domain: "deltoids.dev",
  url: "https://deltoids.dev",
  version: "0.2.0",
  license: "MIT",
  tagline: "Diffs for the agentic era.",
  description:
    "deltoids is a smart diff pager that expands hunks with the context you need to understand them.",
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
  { href: "#pager", label: "Pager" },
  { href: "#agents", label: "Agents" },
  { href: "#install", label: "Install" },
  { href: "#faq", label: "FAQ" },
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

/** edit-tui install snippets per coding agent. */
export const AGENT_TOOLS: { id: string; label: string; code?: string; coming?: boolean }[] = [
  {
    id: "pi",
    label: "pi",
    code: `pi install https://github.com/juanibiapina/deltoids`,
  },
  { id: "claude", label: "Claude Code", coming: true },
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
    code: `brew install juanibiapina/taps/deltoids-cli
brew install juanibiapina/taps/edit-cli`,
  },
  {
    id: "shell",
    label: "shell installer",
    code: "curl -sSL https://github.com/juanibiapina/deltoids/releases/latest/download/deltoids-cli-installer.sh | sh",
  },
  {
    id: "cargo",
    label: "cargo (from source)",
    code: `cargo install --git https://github.com/juanibiapina/deltoids deltoids-cli
cargo install --git https://github.com/juanibiapina/deltoids edit-cli`,
  },
];

/** FAQ. */
export const FAQ: { q: string; a: string }[] = [
  {
    q: "How is this different from `git diff -W`?",
    a: "`git diff -W` finds scope with regex. `deltoids` parses the file with tree-sitter.",
  },
];

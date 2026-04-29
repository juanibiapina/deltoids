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
    "deltoids expands every hunk in a unified diff to include the entire enclosing context. Tree-sitter resolved. Pipe git diff, gh pr diff, or set as your pager.",
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
] as const;

export type Tool = {
  id: string;
  label: string;
  language: "bash" | "yaml" | "toml";
  code: string;
};

/** `git diff | deltoids` snippets per tool. Carried from web/Pager.tsx. */
export const PAGER_TOOLS: Tool[] = [
  {
    id: "git",
    label: "git",
    language: "bash",
    code: `# pipe a single diff
git diff | deltoids | less -R

# or set as the default git pager
git config --global core.pager 'deltoids | less -R'`,
  },
  {
    id: "gh",
    label: "gh",
    language: "bash",
    code: `gh pr diff <number> | deltoids | less -R`,
  },
  {
    id: "lazygit",
    label: "lazygit",
    language: "yaml",
    code: `# ~/.config/lazygit/config.yml
git:
  paging:
    pager: deltoids`,
  },
];

/** edit-tui install snippets per coding agent. Carried from web/Agents.tsx. */
export const AGENT_TOOLS: { id: string; label: string; code?: string; coming?: boolean }[] = [
  {
    id: "pi",
    label: "pi",
    code: `pi install https://github.com/juanibiapina/deltoids
edit-tui`,
  },
  { id: "claude", label: "Claude Code", coming: true },
];

export type InstallCard = {
  id: string;
  label: string;
  code: string;
  note?: string;
};

/**
 * Verified install paths:
 * - brew formulas live at github.com/juanibiapina/homebrew-taps
 * - shell installer URL pattern is what cargo-dist emits per release
 * - cargo install --git is the documented from-source path in README
 */
export const INSTALL_CARDS: InstallCard[] = [
  {
    id: "homebrew-deltoids",
    label: "homebrew",
    code: "brew install juanibiapina/taps/deltoids-cli",
    note: "deltoids diff pager. macOS and Linux.",
  },
  {
    id: "homebrew-edit",
    label: "homebrew (edit-tui)",
    code: "brew install juanibiapina/taps/edit-cli",
    note: "edit, write, and edit-tui — for tracing agent edits.",
  },
  {
    id: "shell",
    label: "shell installer",
    code: "curl -sSL https://github.com/juanibiapina/deltoids/releases/latest/download/deltoids-cli-installer.sh | sh",
    note: "Prebuilt binaries from GitHub Releases.",
  },
  {
    id: "cargo",
    label: "cargo (from source)",
    code: `cargo install --git https://github.com/juanibiapina/deltoids deltoids-cli
cargo install --git https://github.com/juanibiapina/deltoids edit-cli`,
  },
];

/** FAQ from web/Faq.tsx, kept verbatim. */
export const FAQ: { q: string; a: string }[] = [
  {
    q: "How is this different from `git diff -W`?",
    a: "`git diff -W` finds scope with regex. `deltoids` parses the file with tree-sitter.",
  },
];

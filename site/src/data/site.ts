/**
 * Single source of truth for everything that might change between
 * releases or that appears in more than one component. Update here, do
 * not hardcode in templates.
 */

export const SITE = {
  name: "deltoids",
  domain: "deltoids.dev",
  url: "https://deltoids.dev",
  version: "0.1.0",
  license: "MIT",
  tagline: "Diffs for the agentic era.",
  description:
    "Tree-sitter-aware diff pager. Expands every hunk to include the enclosing context. Pipe git diff, gh pr diff, or set as your default pager.",
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
  { href: "#features", label: "Features" },
  { href: "#install", label: "Install" },
  { href: "#faq", label: "FAQ" },
] as const;

export type Tool = {
  id: string;
  label: string;
  language: "bash" | "yaml" | "toml";
  code: string;
};

/** `git diff | deltoids` snippets per tool. */
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

/** `edit-tui` install snippets per coding agent. */
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

export const INSTALL_CARDS: InstallCard[] = [
  {
    id: "homebrew",
    label: "homebrew",
    code: "brew install juanibiapina/taps/deltoids-cli",
    note: "macOS / Linux. Installs the deltoids diff pager.",
  },
  {
    id: "homebrew-edit",
    label: "homebrew (edit-tui)",
    code: "brew install juanibiapina/taps/edit-cli",
    note: "Adds edit, write, and edit-tui for AI-agent traces.",
  },
  {
    id: "shell",
    label: "shell installer",
    code: "curl -sSL https://github.com/juanibiapina/deltoids/releases/latest/download/deltoids-cli-installer.sh | sh",
    note: "Prebuilt binaries from GitHub Releases (linux + macOS, x86_64 / aarch64).",
  },
  {
    id: "cargo",
    label: "cargo",
    code: `cargo install --git https://github.com/juanibiapina/deltoids deltoids-cli
cargo install --git https://github.com/juanibiapina/deltoids edit-cli`,
    note: "Build from source. Requires Rust 1.85+.",
  },
];

export const FAQ: { q: string; a: string }[] = [
  {
    q: "How is this different from `git diff -W`?",
    a: "`git diff -W` finds scope with regex. `deltoids` parses the file with tree-sitter, so it understands functions, classes, methods, and impls — not just brace-matching.",
  },
  {
    q: "Which languages does it support?",
    a: "Rust, Go, JavaScript, TypeScript, Python, Ruby, Java, C, C++, and a few more. Anything without a tree-sitter parser falls back to a 3-line context, identical to plain `diff -U3`.",
  },
  {
    q: "Does it modify the diff content?",
    a: "No. `deltoids` only changes which lines are shown. The added / removed lines are exactly what `git diff` produced; only the context window grows to include the enclosing scope.",
  },
  {
    q: "Will it work as my default git pager?",
    a: "Yes. `git config --global core.pager 'deltoids | less -R'` and every `git diff`, `git log -p`, `git show` runs through it. Same for lazygit, gh, and any tool that pipes unified diff to a pager.",
  },
];

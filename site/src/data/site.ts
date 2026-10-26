/**
 * Single source of truth. Update here, do not hardcode.
 */

export const SITE = {
  name: "deltoids",
  domain: "deltoids.dev",
  url: "https://deltoids.dev",
  reviewUrl: "https://review.deltoids.dev",
  version: "0.16.0",
  license: "MIT",
  tagline: "Diffs for the agentic era.",
  description:
    "deltoids is a code review toolkit: a terminal TUI, a diff pager, and a browser PR reviewer, with hunks that expand to their enclosing scope.",
  heroSub:
    "Review code in your terminal. Hunks grow to their enclosing function, Jev points you at the changes that matter, and language servers take you to definitions and callers.",
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
  { href: "/#review", label: "Review" },
  { href: "/#pager", label: "Pager" },
  { href: SITE.reviewUrl, label: "Try it live" },
  { href: "/#install", label: "Install" },
  { href: "/#faq", label: "FAQ" },
  { href: "/docs/", label: "Docs" },
] as const;

export type ReviewFeature = {
  /** Key or gesture shown as a chip on the card. */
  keys: string;
  title: string;
  body: string;
  href: string;
};

/** TUI feature cards in the Review section. Each links to its docs page. */
export const REVIEW_FEATURES: ReviewFeature[] = [
  {
    keys: "1 2 0",
    title: "Panels like lazygit",
    body: "Files and branches on the left, the diff on the right. Stage, unstage, and discard without leaving the review.",
    href: "/docs/tui/",
  },
  {
    keys: "z / x",
    title: "Context on demand",
    body: "Hunks show their enclosing function. Grow one to its class or the whole file, and shrink it back.",
    href: "/docs/tui/#hunk-context",
  },
  {
    keys: "Enter",
    title: "Commits and branches",
    body: "Open any commit, or a whole branch, as one diff with the same review tools as the working tree.",
    href: "/docs/tui/history/",
  },
  {
    keys: "but",
    title: "GitButler workspaces",
    body: "See stacks, branches, and commits like `but status`. Discards go through `but`, so `but undo` brings them back.",
    href: "/docs/tui/history/#gitbutler",
  },
  {
    keys: "f / F",
    title: "Review guidance",
    body: "TypeSafe's Jev tags each change and marks the ones that need care. Hide tests, comments, and imports.",
    href: "/docs/tui/guidance/",
  },
  {
    keys: "Enter / r",
    title: "Code navigation",
    body: "Go to a definition or list a function's callers from the diff, through rust-analyzer, gopls, and other language servers.",
    href: "/docs/tui/navigation/",
  },
  {
    keys: "c / y",
    title: "Comments for agents",
    body: "Comment on diff lines, then copy every note with its file, line, and code to hand to a coding agent.",
    href: "/docs/tui/comments/",
  },
  {
    keys: "t",
    title: "Syntax themes",
    body: "Pick from Tokyo Night and 32 more bundled themes, live, or set a default in `config.toml`.",
    href: "/docs/configuration/#theme",
  },
];

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
    a: "No. Piped a diff, it's a pager. Run in a terminal, `deltoids` opens a TUI to review your working tree, your branches, and their commits, in git or in a GitButler workspace.",
  },
  {
    q: "Does `deltoids` send my code anywhere?",
    a: "Only when you set `TYPESAFE_API_KEY`. Then the TUI sends the changed hunks to TypeSafe's Jev for review guidance. Lockfiles and generated files stay local. Language servers run on your machine.",
  },
  {
    q: "Which languages support go to definition?",
    a: "Rust, Go, TypeScript, JavaScript, Python, C, C++, Ruby, Lua, Bash, Java, and Kotlin work with their usual language server on `PATH`. The other languages deltoids parses, such as CSS, JSON, and YAML, can get a server in `config.toml`. See <a href=\"/docs/configuration/#language-servers\">Language servers</a>.",
  },
  {
    q: "How is this different from `git diff -W`?",
    a: "`git diff -W` finds scope with regex. `deltoids` parses the file with tree-sitter.",
  },
  {
    q: "Which languages get syntax highlighting?",
    a: "Any syntax bundled with syntect (e.g. Dockerfile), independent of tree-sitter scope support. Scope expansion covers the tree-sitter languages.",
  },
];

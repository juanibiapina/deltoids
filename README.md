# deltoids

> **Beta**: This project is under active development. APIs and behavior may change. Diff output may still be broken; verify important changes.

Tools for reviewing code in the agentic era.

<table>
  <tr>
    <td valign="top"><img src="docs/images/delta.png" alt="Default: 3 lines of context"></td>
    <td valign="top"><img src="docs/images/deltoids.png" alt="deltoids: hunk expanded to enclosing function"></td>
  </tr>
  <tr>
    <td align="center"><em>default</em></td>
    <td align="center"><em>deltoids</em></td>
  </tr>
</table>

Hunks expand to show the enclosing function, so you always know where you are.

## Overview

The core idea of this project is to make diffs more powerful. Diffs produced by all tools have syntax highlighting and word-level highlight within changed lines. They also expand to include relevant context, usually the enclosing function or struct up to 200 lines. This allows you to quickly view the entire context without having to switch to an editor.

This project contains a collection of tools. The main tool is `deltoids`, a git pager inspired by `delta` and `difftastic`.

Another set of tools is `edit`, `write` and `edit-tui`. `edit` and `write` are CLI versions of AI coding agent tools. By providing these custom CLIs, we can tell coding agents to generate summaries for each change and visualize them with `edit-tui` separately from the coding agent UI.

## Installation

**Homebrew:**

```bash
brew install juanibiapina/taps/deltoids-cli
brew install juanibiapina/taps/edit-cli
```

**Prebuilt binaries (shell installer):**

```bash
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/juanibiapina/deltoids/releases/latest/download/deltoids-cli-installer.sh | sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/juanibiapina/deltoids/releases/latest/download/edit-cli-installer.sh | sh
```

**From source (cargo):**

```bash
cargo install --git https://github.com/juanibiapina/deltoids deltoids-cli
cargo install --git https://github.com/juanibiapina/deltoids edit-cli
```

This installs:
- `deltoids`: diff viewer
- `edit`: file edit tool (used by coding agents)
- `write`: file write tool (used by coding agents)
- `edit-tui`: trace browser to follow agents in real time

## Usage

### Standalone

Pipe any unified diff through `deltoids`:

```bash
git diff | deltoids | less -R
git show HEAD~1 | deltoids | less -R
git log -p | deltoids | less -R
```

### Git Integration

Set `deltoids` as your default pager:

```bash
git config --global core.pager 'deltoids | less -R'
```

Or for a specific command:

```bash
git config --global pager.diff 'deltoids | less -R'
git config --global pager.show 'deltoids | less -R'
git config --global pager.log 'deltoids | less -R'
```

### Lazygit Integration

Add to `~/.config/lazygit/config.yml`:

```yaml
git:
  paging:
    pager: deltoids
```

## Coding Agent Integrations

### pi

Install the pi package to override built-in `edit` and `write` tools with the traced versions:

```bash
pi install https://github.com/juanibiapina/deltoids
```

Requires `edit` and `write` binaries on PATH. See [plugins/pi/README.md](plugins/pi/README.md) for details.

Then open `edit-tui` in the same directory as pi to see real-time diffs with summaries.

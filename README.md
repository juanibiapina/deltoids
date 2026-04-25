# deltoids

> **Beta**: This project is under active development. APIs and behavior may change.

Tools for reviewing code in the agentic era.

Website: <https://deltoids.dev>

## Installation

**From source:**

```bash
git clone https://github.com/juanibiapina/deltoids.git
cd deltoids

# Install all binaries
cargo install --path crates/deltoids-cli
cargo install --path crates/edit-cli
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
git diff | deltoids
git show HEAD~1 | deltoids
git log -p --color=always | deltoids
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

Then open `edit-tui` in the same directory as pi to see real time diffs with summaries.

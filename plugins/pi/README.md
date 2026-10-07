# Pi Integration

This directory contains the pi extension for deltoids.

## Requirements

- The `deltoids` binary must be installed and available on PATH
- Optional: [Kao](https://github.com/juanibiapina/kao) on PATH, to record file
  changes made by `bash` commands

Install it with:

```bash
cargo install --path crates/deltoids-cli
```

## Install

```bash
pi install https://github.com/juanibiapina/deltoids
```

## What It Does

The extension overrides pi's built-in `edit`, `write`, and `bash` tools with traced versions. This enables:

- **Trace tracking**: All edits are recorded in traces at `$XDG_DATA_HOME/edit/traces/`
- **Trace continuity**: Trace IDs persist across tool calls within a session
- **TUI browser**: Review changes with `deltoids tui` (opens on the working-tree diff; press `[`/`]` to toggle to the trace browser)

Each `edit` call replaces one exact region using
`{ reason, path, oldText, newText }`. `oldText` must match the file's
current text exactly and appear exactly once. To make several changes,
the model issues several `edit` calls against the file's current text.
Full-file rewrites through `write` are recorded in the same trace.

## How It Works

1. When the agent calls `edit` or `write`, the extension spawns the
   matching `deltoids` subcommand.
2. The request is piped as JSON to stdin.
3. The response, including the trace ID and diff, is captured from stdout.
4. Trace IDs are stored in session state and reused for subsequent calls.

### Bash commands

Inside a Git working tree with `kao` installed, `bash` runs commands
through `kao run`, which captures every file the command changed. The
extension then imports that capture with `deltoids record`, so one command
becomes one trace entry listing all its files. Read-only commands record
nothing. Output, exit codes, timeouts, and aborts behave as before; an
aborted or timed-out command still records what it changed.

`edit` and `write` run under `kao lock`, so they wait for a running
command instead of landing inside its capture. Kao's lock only covers
processes started through Kao: editors, servers started outside Kao, and
background jobs (`&`) can still write during a command, and their changes
show up in its entry.

Every entry records the pi session and tool call that made it.

Outside a Git working tree, or without `kao`, every tool runs as before and
`bash` changes are not recorded.

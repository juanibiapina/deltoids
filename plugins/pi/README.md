# Pi Integration

This directory contains the pi extension for deltoids.

## Requirements

- `edit` and `write` binaries must be installed and available on PATH

Install them with:

```bash
cargo install --path crates/edit-cli
```

## Install

```bash
pi install https://github.com/juanibiapina/deltoids
```

## What It Does

The extension overrides pi's built-in `edit` and `write` tools to use the external CLI binaries instead. This enables:

- **Trace tracking**: All edits are recorded in traces at `$XDG_DATA_HOME/edit/traces/`
- **Trace continuity**: Trace IDs persist across tool calls within a session
- **TUI browser**: Review traces with `edit-tui`

## How It Works

1. When the agent calls `edit` or `write`, the extension spawns the external binary
2. The request is piped as JSON to stdin
3. The response (including trace ID and diff) is captured from stdout
4. Trace IDs are stored in session state and reused for subsequent calls

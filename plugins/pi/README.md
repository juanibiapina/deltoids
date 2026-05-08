# Pi Integration

This directory contains the pi extension for deltoids.

## Requirements

- The `deltoids` binary must be installed and available on PATH

Install it with:

```bash
cargo install --path crates/deltoids-cli
```

## Install

```bash
pi install https://github.com/juanibiapina/deltoids
```

## What It Does

The extension overrides pi's built-in `edit` and `write` tools to use the external CLI binaries instead. This enables:

- **Trace tracking**: All edits are recorded in traces at `$XDG_DATA_HOME/edit/traces/`
- **Trace continuity**: Trace IDs persist across tool calls within a session
- **TUI browser**: Review traces with `deltoids traces`

## How It Works

1. When the agent calls `edit` or `write`, the extension spawns `deltoids edit` or `deltoids write`
2. The request is piped as JSON to stdin
3. The response (including trace ID and diff) is captured from stdout
4. Trace IDs are stored in session state and reused for subsequent calls

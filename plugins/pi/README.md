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

The extension overrides pi's built-in `edit` and `write` tools in both modes — the `edit` tool's schema and backend differ depending on the mode (see below) — and in hash mode also adds a new `hashread` tool. This enables:

- **Trace tracking**: All edits are recorded in traces at `$XDG_DATA_HOME/edit/traces/`
- **Trace continuity**: Trace IDs persist across tool calls within a session
- **TUI browser**: Review traces with `deltoids traces`

## Modes

The extension supports two self-contained tool sets, selected once at
extension load by the `DELTOIDS_EDIT_MODE` env var. The mode is fixed
for the lifetime of the pi session — changing it requires restarting
pi. This keeps the system prompt static (so pi's prompt caching stays
warm) and the tool surface deterministic.

- **`text`** (default): overrides pi's built-in `edit` with a
  `deltoids edit` pass-through (`oldText` / `newText` replacement).
  Pi's built-in `read` is used for everything.

- **`hash`**: overrides pi's built-in `edit` with a `deltoids hashedit`
  pass-through. The model still sees the tool as `edit`; only the
  schema is the line-anchored shape (ops `replace`, `insert_before`,
  `insert_after`, `delete` with `LINEhh` anchors). A stale anchor
  rejects the whole batch with fresh-anchor context. Adds `hashread`
  as a new tool for reading text files with anchors. Pi's built-in
  `read` is not overridden — it remains the right choice for images,
  directories, URLs, archives, SQLite, and other non-text content
  `hashread` cannot handle; prompt guidelines steer the model toward
  `hashread` for text-file reads.

`write` is overridden in both modes (full-file rewrites land in the
trace either way).

Enable hash mode by exporting the env var before launching pi:

```bash
export DELTOIDS_EDIT_MODE=hash
pi
```

The footer shows the current mode (e.g. `deltoids: hash`).

## How It Works

1. When the agent calls `edit`, `write`, or `hashread`, the extension
   spawns the matching `deltoids` subcommand: in text mode `edit`
   spawns `deltoids edit`; in hash mode `edit` spawns `deltoids
   hashedit`. `write` always spawns `deltoids write`. `hashread`
   always spawns `deltoids hashread`.
2. The request is piped as JSON to stdin.
3. The response (including trace ID and diff) is captured from stdout.
   For `hashread` the response is the formatted text body, not JSON.
4. Trace IDs are stored in session state and reused for subsequent calls
   (covers `edit` in both modes and `write`).

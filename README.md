# edit

CLI tools that trace `edit` and `write` file changes, with a TUI to browse traces.

## edit input

```json
{
  "summary": "Update x constant",
  "path": "src/app.ts",
  "edits": [
    {
      "summary": "Update x constant",
      "oldText": "const x = 1;",
      "newText": "const x = 2;"
    }
  ]
}
```

## write input

```json
{
  "summary": "Rewrite config",
  "path": "config.json",
  "content": "{\n  \"version\": 2\n}\n"
}
```

## features

- `edit` requires top-level `summary` and per-edit `summary`.
- `write` rewrites full file contents and returns a diff.
- Success and failure responses include `traceId` when the request was parsed.
- Successful and failed attempts are appended to:
  - `$XDG_DATA_HOME/edit/traces/<trace-id>/entries.jsonl`
  - fallback: `~/.local/share/edit/traces/<trace-id>/entries.jsonl`
- `edit` and `write` can share the same trace id.
- If you pass a trace id, it must be an existing ULID trace id to reuse. Omit it to start a new trace.
- `edit` shorthand:
  - `edit [trace-id] --path src/app.ts --summary "Rename x" --old "const x = 1;" --new "const count = 1;"`
- `write` shorthand:
  - `write [trace-id] --path config.json --summary "Rewrite config" < config.json.new`

See [docs/delta-within-line-diff-reference.md](docs/delta-within-line-diff-reference.md) for the reference algorithm used to match delta's within-line diff behavior.

## diff scope context

The TUI displays scope context (enclosing function, class, etc.) for each change, powered by tree-sitter. This helps navigate changes in context.

Supported languages: Rust, Python, JavaScript, TypeScript (including TSX), Go, Ruby, Java, C, C++, Bash, Lua, CSS, and HCL/Terraform.

## edit-tui

Run `edit-tui` in a directory to browse the traces produced from that directory.

Layout (lazygit-inspired):

- Left sidebar, top: entries (edits/writes) of the selected trace.
- Left sidebar, bottom: traces for the current working directory.
- Right: detail for the selected entry, including a combined header block with summary, path, and metadata, followed by orange summary blocks, blue hunk headers, and diff.

The view auto-refreshes when traces change on disk, so new edits from other processes appear without restarting.

Keys:

- `Tab`: cycle focus across entries, traces, and diff.
- `1` / `2` / `3`: focus the entries, traces, or diff pane directly.
- `j` / `k` / arrows: move within the focused pane (scrolls the diff when it is focused).
- `Shift+J` / `Shift+K`: scroll the diff pane without leaving the entries or traces pane.
- `PgUp` / `PgDn`: page-scroll the diff pane regardless of focus.
- `q` / `Esc`: quit.

Visual aids:

- Each pane has an index prefix in its title (`[1] Entries`, `[2] Traces`, `[3] Diff`).
- List panes show a `N of M` position in the bottom-right corner.
- The diff pane starts with a combined header block that makes the summary, path, and entry metadata easy to scan.
- The diff pane renders a vertical scrollbar on the right edge when the content overflows.
- Orange summary blocks are separate from blue source-location hunk headers.

## examples

```bash
printf '%s' '{
  "summary": "Update x constant",
  "path": "src/app.ts",
  "edits": [
    {
      "summary": "Update x constant",
      "oldText": "const x = 1;",
      "newText": "const x = 2;"
    }
  ]
}' | edit
```

```bash
printf '%s' '{
  "summary": "Rewrite config",
  "path": "config.json",
  "content": "{\n  \"version\": 2\n}\n"
}' | write
```

## quality checks

Clippy is configured at the workspace root so both crates share the same lint policy.

Run the current Clippy baseline from the repo root:

```bash
cargo clippy --workspace --all-targets
```

This is report-only for now. Do not add `-- -D warnings` until the current findings are cleaned up.

To collect code metrics from the repo root, install `rust-code-analysis-cli` and run:

```bash
cargo install --locked rust-code-analysis-cli
rust-code-analysis-cli --metrics \
  -p src \
  -p deltoids/src \
  -p tests \
  -p deltoids/tests \
  --output-format json \
  --pr
```

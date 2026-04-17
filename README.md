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

Diff hunk headers include the enclosing source line, powered by tree-sitter.
For example, a change inside `fn compute(&self) -> i32 {` produces:

```
@@ -13,7 +13,7 @@ fn compute(&self) -> i32 {
```

Supported languages: Rust, Python, JavaScript, TypeScript (including TSX), Go, Ruby, Java, C, C++, Bash, Lua, CSS, and HCL/Terraform. Files with unrecognized extensions produce standard hunk headers without scope context.

## edit-tui

Run `edit-tui` in a directory to browse the traces produced from that directory.

Layout (lazygit-inspired):

- Left sidebar, top: entries (edits/writes) of the selected trace.
- Left sidebar, bottom: traces for the current working directory.
- Right: detail for the selected entry, including top-level summary, path, blue hunk headers, and diff.

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
- The diff pane renders a vertical scrollbar on the right edge when the content overflows.

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

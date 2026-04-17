# edit

CLI tools that trace `edit` and `write` file changes.

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
- Trace commands:
  - `edit traces list`
  - `edit traces list <trace-id>`
  - `edit traces show <trace-id> <index>`
  - `edit traces review <trace-id>` opens a terminal UI with syntax-highlighted diffs.

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

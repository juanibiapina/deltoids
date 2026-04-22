# edit

CLI tools for tracing file edits, with a TUI to browse traces.

## Crates

| Crate | Description |
|-------|-------------|
| `edit` | Core library for trace management |
| `edit-cli` | `edit`, `write`, and `edit-tui` binaries |
| `deltoids` | Diff library with tree-sitter scope context |
| `deltoids-cli` | `deltoids` diff filter binary |

## Install

```bash
cargo install --path crates/edit-cli      # edit, write, edit-tui
cargo install --path crates/deltoids-cli  # deltoids
```

## edit

Applies targeted text replacements to a file.

```bash
printf '%s' '{
  "summary": "Update constant",
  "path": "src/app.ts",
  "edits": [
    {
      "summary": "Change x to 2",
      "oldText": "const x = 1;",
      "newText": "const x = 2;"
    }
  ]
}' | edit
```

Shorthand:
```bash
edit --path src/app.ts --summary "Change x" --old "const x = 1;" --new "const x = 2;"
```

## write

Rewrites a file with full content.

```bash
printf '%s' '{
  "summary": "Rewrite config",
  "path": "config.json",
  "content": "{\n  \"version\": 2\n}\n"
}' | write
```

Shorthand:
```bash
write --path config.json --summary "Rewrite config" < new_config.json
```

## Traces

Both commands log to `$XDG_DATA_HOME/edit/traces/<trace-id>/entries.jsonl`.

- Omit trace id to start a new trace
- Pass an existing trace id to append: `edit <trace-id> ...`
- `edit` and `write` can share the same trace

## edit-tui

Browse traces for the current directory.

```
┌─[1] Entries 1 of 3─────┬─[3] Diff─────────────────────────┐
│ ✓ Update constant      │ src/app.ts                       │
│ ✓ Rewrite config       │ edit • ok • 1 edit • 1 hunk      │
│ ✗ Failed edit          │──────────────────────────────────│
│                        │ ┌─ 1: foo()                      │
├─[2] Traces 1 of 2──────┤ │                                │
│ > 01HX... app.ts       │ -const x = 1;                    │
│   01HW... config.json  │ +const x = 2;                    │
└────────────────────────┴──────────────────────────────────┘
```

Keys:
- `Tab` / `1` `2` `3`: switch panes
- `j` `k` / arrows: navigate
- `Shift+J` `Shift+K`: scroll diff from any pane
- `q`: quit

Auto-refreshes when traces change on disk.

## deltoids

Diff filter with tree-sitter scope context. Shows enclosing function/class as breadcrumbs.

```bash
git diff | deltoids
git show | deltoids
```

Supported: Rust, Python, JavaScript, TypeScript, Go, Ruby, Java, C, C++, Bash, Lua, CSS, HCL, Markdown, TOML, JSON, YAML.

## Development

```bash
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets
cargo fmt --all -- --check
```

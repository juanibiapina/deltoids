# edit

CLI for agents to edit files.

## Overview

Run `edit` with no stdin to print an agent-friendly overview with usage, rules, and an example.

## Input

When stdin contains non-empty JSON, the command reads one JSON object:

```json
{
  "summary": "Update x constant",
  "path": "src/app.ts",
  "edits": [
    {
      "oldText": "const x = 1;",
      "newText": "const x = 2;"
    }
  ]
}
```

## Behavior

- `summary` is required and must not be empty.
- `path` must point to an existing UTF-8 text file.
- `edits` must contain at least one replacement.
- Request field names are exact JSON keys: `oldText` and `newText`.
- Unknown fields are rejected.
- Each `oldText` must match exactly once in the original file.
- All matches are resolved against the original file contents, not incrementally.
- Requested edit regions must not overlap.
- All edits are validated before any write happens.
- The file is not modified if any edit fails.
- Missing paths fail with `Path does not exist: <path>`.
- Directory and other non-file paths fail with `Path is not a file: <path>`.
- Success responses include a line-based `diff` string.

## Example

```bash
printf '%s' '{
  "summary": "Update x constant",
  "path": "src/app.ts",
  "edits": [
    {
      "oldText": "const x = 1;",
      "newText": "const x = 2;"
    }
  ]
}' | edit
```

Success writes a JSON response to stdout:

```json
{"ok":true,"path":"src/app.ts","replacedBlocks":1,"diff":"--- original\n+++ modified\n@@ -1 +1 @@\n-const x = 1;\n+const x = 2;\n"}
```

Failure writes a JSON response to stderr and exits non-zero:

```json
{"ok":false,"error":"Could not find edits[0] in src/app.ts. The oldText must match exactly."}
```

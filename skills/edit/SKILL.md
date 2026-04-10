---
name: edit
description: Use this tool to edit one file with exact text replacement. Best for precise, surgical changes where oldText can match the file exactly.
---

# Edit

Use this CLI via bash as `edit`. It reads one JSON request from stdin and edits one file with exact text replacement.

## Input

Invoke `edit` and pipe one JSON object to stdin with:
- `summary`: short description of the change. Required. Must not be empty.
- `path`: file to edit. Must exist.
- `edits`: one or more replacements.

Each edit must use:
- `oldText`
- `newText`

## Rules

- `oldText` must match exactly, including whitespace and newlines.
- Each `oldText` must match exactly once in the original file.
- All edits are matched against the original file, not after earlier edits are applied.
- Edit regions must not overlap.
- If two changes touch the same block or nearby lines, merge them into one edit.
- Keep `oldText` as small as possible while still unique.
- Do not pad `oldText` with large unchanged regions.
- Unknown JSON fields are rejected.
- If any edit fails, nothing is written.

## Example

```bash
printf '%s' '{
  "summary": "Rename variable",
  "path": "src/app.ts",
  "edits": [
    {
      "oldText": "const x = 1;",
      "newText": "const count = 1;"
    }
  ]
}' | edit
```

## Output

Success goes to stdout:

```json
{"ok":true,"path":"src/app.ts","replacedBlocks":1,"diff":"--- original\n+++ modified\n@@ -1 +1 @@\n-const x = 1;\n+const count = 1;\n"}
```

Failure goes to stderr and exits non-zero:

```json
{"ok":false,"error":"Could not find edits[0] in src/app.ts. The oldText must match exactly."}
```

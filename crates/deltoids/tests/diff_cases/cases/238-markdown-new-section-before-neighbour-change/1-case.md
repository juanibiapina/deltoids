# New Markdown section before a neighbouring change

## Why this case exists

Two edits sit a few lines apart: a new section is inserted above a bullet added to an existing section. The inserted section forms a new scope, so it is planned as an unmergeable hunk of its own. The bullet change uses default three-line context, whose range would otherwise span the insertion point.

A hunk's lines must be contiguous in both files because every consumer numbers them by walking from `old_start` and `new_start`.

## Behaviours pinned

- The new `### Added` section is one hunk of its own.
- The bullet added to `### Changed` is a separate hunk whose new-file start accounts for the inserted lines.
- Hunks are emitted in file order.
- `### Changed` is the shared boundary between the two hunks. A single shared context line is expected, but neither hunk spans content owned by the other.

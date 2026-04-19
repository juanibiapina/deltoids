# deltoids

A library for computing structural scope context from diffs using tree-sitter. Given original and updated file content, deltoids generates a diff enriched with scope information (functions, classes, modules, etc.) for each hunk.

The library produces data, not presentation. Consumers own their rendering.

## Supported languages

Rust, Python, JavaScript, TypeScript, TSX, Go, Ruby, Java, C, C++, Bash, Lua, CSS, HCL/Terraform.

## API

### `Diff`

The main entry point. Computes a diff and enriches it with tree-sitter scope information.

```rust
use deltoids::Diff;

let original = "fn foo() {\n    1\n}\n";
let updated = "fn foo() {\n    2\n}\n";

let diff = Diff::compute(original, updated, "test.rs");

// Get plain diff text with standard 3-line context (for agents)
let plain = diff.text();

// Get diff with scope context in @@ headers
let with_scope = diff.text_with_scope();
// @@ -1,3 +1,3 @@ fn foo() {

// Get structured hunks with scope-expanded context (for TUI)
let hunks = diff.hunks();
```

#### Scope-expanded context

Hunks returned by `hunks()` use scope-expanded context:

- If the innermost enclosing scope is ≤50 lines, the hunk includes the full scope
- Scopes >50 lines fall back to standard 3-line context
- Changes in the same scope are merged into a single hunk
- Changes in different scopes produce separate hunks

The `text()` method always returns standard 3-line unified diff format.

### `Hunk`

A parsed diff hunk with scope information.

```rust
pub struct Hunk {
    pub old_start: usize,
    pub new_start: usize,
    pub lines: Vec<DiffLine>,
    pub ancestors: Vec<ScopeNode>,
}
```

- `old_start`, `new_start`: 1-indexed line numbers from the hunk header
- `lines`: The diff lines (context, added, removed)
- `ancestors`: Enclosing scopes from outermost to innermost

### `DiffLine`

A single line in a hunk.

```rust
pub struct DiffLine {
    pub kind: LineKind,
    pub content: String,
}

pub enum LineKind {
    Added,
    Removed,
    Context,
}
```

### `ScopeNode`

One structural ancestor in a scope chain.

```rust
pub struct ScopeNode {
    pub kind: String,
    pub name: String,
    pub start_line: usize,
    pub end_line: usize,
    pub text: String,
}
```

- `kind`: Tree-sitter node type (e.g., `"function_item"`, `"impl_item"`)
- `name`: Extracted identifier (e.g., `"Config"` for an impl block)
- `start_line`, `end_line`: 1-indexed line range
- `text`: The opening source line with original indentation preserved

## Examples

### Get scope-enriched diff for display

```rust
use deltoids::Diff;

let original = std::fs::read_to_string("src/config.rs").unwrap();
let updated = apply_my_changes(&original);

let diff = Diff::compute(&original, &updated, "src/config.rs");
println!("{}", diff.text_with_scope());
// @@ -14,7 +14,7 @@ fn process(&self) -> Result {
//  ...
```

### Inspect scope chains

```rust
use deltoids::Diff;

let original = "\
struct Foo;

impl Foo {
    fn compute(&self) -> i32 {
        let x = 1;
        x + 1
    }
}
";
let updated = original.replace("x + 1", "x + 2");

let diff = Diff::compute(original, &updated, "test.rs");

for hunk in diff.hunks() {
    println!("hunk at line {}", hunk.new_start);
    for ancestor in &hunk.ancestors {
        println!(
            "  {} {} (lines {}-{})",
            ancestor.kind, ancestor.name, ancestor.start_line, ancestor.end_line
        );
    }
}
// hunk at line 3
//   impl_item Foo (lines 3-8)
//   function_item compute (lines 4-7)
```

### Store hunks for later use

```rust
use deltoids::{Diff, Hunk};

let diff = Diff::compute(original, updated, path);
let hunks: Vec<Hunk> = diff.hunks().to_vec();

// Serialize for storage
let json = serde_json::to_string(&hunks).unwrap();
```

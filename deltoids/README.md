# deltoids

A library for computing structural scope context from diffs using tree-sitter. Given a unified diff and source files, deltoids determines which functions, classes, modules, and other structural boundaries enclose each change.

The library produces data, not presentation. Consumers own their rendering.

## Supported languages

Rust, Python, JavaScript, TypeScript, TSX, Go, Ruby, Java, C, C++, Bash, Lua, CSS, HCL/Terraform.

## API

#### `deltoids::scope::ScopeNode`

```rust
pub struct ScopeNode {
    pub kind: String,
    pub name: String,
    pub start_line: usize,
    pub end_line: usize,
    pub text: String,
}
```

One structural ancestor in a scope chain. `kind` is the tree-sitter node type. `name` is the extracted identifier (e.g. `"Config"` for an impl block, `"process"` for a function). `start_line` and `end_line` are 1-indexed. `text` is the opening source line with original indentation preserved. Serializable with serde.

#### `deltoids::scope::HunkScopes`

```rust
pub struct HunkScopes {
    pub hunk_old_start: usize,
    pub hunk_new_start: usize,
    pub ancestors: Vec<ScopeNode>,
}
```

Scope data for one diff hunk. `ancestors` is ordered outermost to innermost. Serializable with serde.

#### `deltoids::scope::compute_hunk_scopes`

```rust
pub fn compute_hunk_scopes(diff: &str, original: &str, path: &str) -> Vec<HunkScopes>
```

Given a unified diff and the original file content, compute the full ancestor scope chain for each hunk. Returns one entry per `@@` header. Returns an empty vec for unsupported languages.

#### `deltoids::scope::inject_scope_context`

```rust
pub fn inject_scope_context(diff: &str, original: &str, path: &str) -> String
```

Append the innermost scope's source line after each `@@` marker in a unified diff. Unsupported languages pass through unchanged.

#### `deltoids::scope::scope_expanded_diff`

```rust
pub fn scope_expanded_diff(original: &str, updated: &str, path: &str) -> String
```

Generate a unified diff where context is expanded to cover the full innermost enclosing scope (up to 50 lines). Falls back to standard 3-line context for scopes larger than 50 lines or unsupported languages.

This function takes the original and updated file contents (not a pre-computed diff) because it needs to recompute hunk boundaries based on scope ranges.

## Usage

### Enrich hunk headers with scope labels

```rust
use deltoids::scope::inject_scope_context;

let diff = std::str::from_utf8(git_diff_output).unwrap();
let original = std::fs::read_to_string("src/config.rs").unwrap();

let enriched = inject_scope_context(diff, &original, "src/config.rs");
// Before: @@ -14,7 +14,7 @@
// After:  @@ -14,7 +14,7 @@ fn process(&self) -> Result {
```

### Get the full scope chain per hunk

```rust
use deltoids::scope::compute_hunk_scopes;

let diff = std::str::from_utf8(git_diff_output).unwrap();
let original = std::fs::read_to_string("src/config.rs").unwrap();

let scopes = compute_hunk_scopes(diff, &original, "src/config.rs");

for hunk in &scopes {
    println!("hunk at line {}", hunk.hunk_new_start);
    for ancestor in &hunk.ancestors {
        println!(
            "  {} {} (lines {}-{})",
            ancestor.kind, ancestor.name, ancestor.start_line, ancestor.end_line
        );
    }
}
// hunk at line 14
//   impl_item Config (lines 10-25)
//   function_item process (lines 13-20)
```

### Generate a scope-expanded diff

```rust
use deltoids::scope::scope_expanded_diff;

let original = std::fs::read_to_string("src/config.rs").unwrap();
let updated = apply_my_changes(&original);

let diff = scope_expanded_diff(&original, &updated, "src/config.rs");
// Context lines cover the full enclosing function instead of the default 3 lines,
// so reviewers see the structural context of each change.
```

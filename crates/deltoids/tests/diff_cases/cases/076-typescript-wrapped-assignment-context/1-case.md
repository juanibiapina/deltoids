# Edit to the value line of a wrapped `const x =\n  expr;`

## Why this case exists

Pins the "expand to a boundary" model for a wrapped assignment whose
value sits on its own line:

```ts
const greeting =
  buildGreeting("hello");
```

The change lands on the value line. There is no enclosing structure, so
the expansion walks the raw ancestor chain (call_expression ->
variable_declarator -> lexical_declaration) and stops just below the
file root, anchoring on the whole `const greeting = …;` statement.

## Behaviours pinned

- A change on the value line of a wrapped assignment expands to cover
  the whole statement, including the `const greeting =` opening line.
- The breadcrumb is empty (the statement is not a named structure).

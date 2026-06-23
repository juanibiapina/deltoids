# Edit to an array argument of a `const … = call(…)` loses its anchor

## Why this case exists

Reproduces a real bug seen on `apps/api/src/AgentContainer.ts` in the
`zero` repo. The edited line is one of several arguments to a
multi-line function call that is assigned to a top-level binding:

```ts
const secretProxy = createSecretProxy(
  ["ANTHROPIC_API_KEY", "BRAVE_API_KEY"],
  ["GOOGLE_WORKSPACE_CLI_TOKEN", "GH_TOKEN"], // <- edited line
);
```

The edited argument is itself a single-line array literal, so the
data-scope expansion that handles case `056` (a multi-line array that
*is* the whole binding value) never kicks in. The enclosing
`call_expression` / `arguments` is neither a recognised structure nor a
data scope, so the hunk falls back to default context and the reader is
shown the bare `["…"]` line with no clue that it is an argument to
`createProxy`, and no view of the `const proxy = createProxy(`
assignment that opens the statement.

This is the same shape as the much larger `056` / `057` literal cases,
but the literal is nested one level deeper — as an argument to a call
rather than as the binding value itself.

## Behaviours pinned

The hunk expands to cover the whole `const proxy = createProxy( … );`
statement so the assignment and the function being called are visible
as context for the changed argument. There is no enclosing structure,
so the expansion walks the raw ancestor chain (array -> arguments ->
call_expression -> variable_declarator -> lexical_declaration) and
stops just below the file root, anchoring on the whole top-level
statement. The breadcrumb is empty (the statement is not a named
structure).

## Notes

This is the canonical case for the "expand to a boundary" model: the
change grows outward through transparent ancestors (call, argument
list, literal) until it reaches the file root, then anchors on the
outermost non-root ancestor that fits the line budget.

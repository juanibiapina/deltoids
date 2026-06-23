# Edit inside a multi-line object passed as a call argument

## Why this case exists

Pins expansion that flows through *two* transparent ancestors at once: a
call and the object literal passed to it, up to the enclosing statement.

```ts
const handler = registerHandler({
  method: "GET",
  path: "/old",
});
```

The change lands on a property of the object argument. The raw ancestor
chain is `pair -> object -> arguments -> call_expression ->
variable_declarator -> lexical_declaration`. With no enclosing
structure, expansion grows outward through every transparent ancestor
and stops just below the file root, anchoring on the whole `const
handler = registerHandler({ … });` statement.

## Behaviours pinned

- Expansion flows through both the call and the object literal up to the
  whole statement; the `const handler = registerHandler({` opening line
  and the callee are visible as context.
- The breadcrumb is empty (the statement is not a named structure).

# TypeScript config object literal has no breadcrumb

## Why this case exists

Many TypeScript files (Astro/Vite/Next/Cypress configs, etc.) are just
nested object literals exported via `defineConfig({...})`. There is no
enclosing function or class that names the change, so the breadcrumb
must be empty even though the engine can see structure in the tree.

## Behaviours pinned

- A change inside a TS export-object literal produces a hunk with no
  ancestors.
- Data-tier scopes (`object`, `pair`) participate in context expansion
  but never appear in the breadcrumb chain.

# Modifying a method on a TypeScript class

## Why this case exists

The TypeScript counterpart of case 045: a change inside a method on a
class should produce a breadcrumb with the class as the outer ancestor
and the method as the inner ancestor.

## Behaviours pinned

- The hunk's breadcrumb is `[class_declaration Greeter]`
  `[method_definition greet]`.
- The hunk uses scope-expanded context covering the method body, not
  just three lines around the change.

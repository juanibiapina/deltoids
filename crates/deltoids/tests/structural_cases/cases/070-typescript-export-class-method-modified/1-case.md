# TypeScript: modify a method inside an exported class

## Why this case exists

For TypeScript, `export class Foo { ... }` lifts visibility on the
class to public; methods inside the class default to public unless
they carry an `accessibility_modifier`. A body-only edit of a method
should report `Modified method` with the qualified path.

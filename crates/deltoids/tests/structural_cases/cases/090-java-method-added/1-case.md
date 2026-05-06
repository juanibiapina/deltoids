# Java: add a private method inside a public class

## Why this case exists

The Java extractor must read the `modifiers` child of
`method_declaration` and emit `Visibility::Private` even when the
enclosing class is public. The `(public)` suffix should NOT appear on
the description for the new method.

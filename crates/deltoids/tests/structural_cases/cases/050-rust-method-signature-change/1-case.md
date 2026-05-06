# Method gains a parameter; signature change reported

## Why this case exists

Adding a parameter to an existing method changes the signature but
leaves the body otherwise. The structural diff must report a
`SignatureChanged` (not just `Modified`) so signature-only views pick
it up.

Method qualification is `Foo::compute` (impl scope qualifier).

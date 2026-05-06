# Python: new method added to an existing class

## Why this case exists

The python extractor must:
- treat `def` inside `class` as a `Method` (not a `Function`),
- qualify it with the class name (`Foo::bar`),
- detect `_private` names as `Visibility::Private` while leaving
  dunder methods (`__init__`) public.

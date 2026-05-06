# Promoting a private function to `pub`

## Why this case exists

A pure visibility change (no signature change, no body change) should
be classified as `VisibilityChanged` with the description showing the
old → new visibility. This is critical for API-review workflows.

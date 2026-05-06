# Renaming a function preserves the function as a single rename

## Why this case exists

When a function is renamed but the parameters, return type, and body
stay the same, signature similarity should kick in and produce one
`Renamed` change rather than `Removed` + `Added`. The arrow form is
the description.

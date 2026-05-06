# Go: add an uppercase (exported) method to a type

## Why this case exists

Go uses initial-case capitalization for visibility. A new method
`(s *Server) Start()` should be reported as `Added method` with
`(public)` suffix; the receiver type doesn't show up in the qualified
path because the Go extractor records methods at file scope (the
language doesn't have C++-style class member functions).

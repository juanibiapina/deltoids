//! Structural ("tree-aware") diff layer.
//!
//! Built on top of the line-level diff in [`crate::scope`] and the
//! tree-sitter parses in [`crate::syntax`]. Walks both ASTs to enumerate
//! named declarations (classes, functions, methods, types, etc.), pairs
//! them by qualified name, and produces a list of [`StructuralChange`]s
//! describing each Added / Removed / Modified / Renamed declaration in
//! human-readable terms ("Added method `Foo::bar`", "Modified function
//! `parse`").
//!
//! The line-level diff stays the backbone for rendering. This module
//! adds **semantic metadata** consumers can use to:
//! - print a high-level summary ("12 changes: +3 functions, -1 class…"),
//! - filter the diff to only public-interface changes,
//! - filter the diff to only signature changes (no bodies),
//! - jump from a change description to the underlying diff hunk.
//!
//! The pipeline borrows ideas from difftastic (content hashing, prefix /
//! suffix shrinking, named-element pairing) but stops short of a full
//! Dijkstra graph search: pairing is name-driven, with similarity-based
//! rename detection as a fallback.

mod classify;
mod diff;
mod pair;
mod symbol;

pub use classify::{ChangeKind, StructuralChange, classify, kind_word};
pub use diff::StructuralDiff;
pub use pair::{Pairing, pair_symbols};
pub use symbol::{LineSpan, Symbol, SymbolKind, SymbolPath, Visibility, extract_symbols};

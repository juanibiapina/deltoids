//! Hashline edit engine — line-anchored editing for coding agents.
//!
//! Each file line is identified by a `(line_number, hash)` anchor. The hash
//! is a 2-character fingerprint computed from the line's content; its only
//! job is to detect "the file changed since you last read it" at apply
//! time. Models address lines by number and quote the hash back so the
//! engine can refuse stale references before mutating the file.
//!
//! ## Why bigrams (alphabet choice)
//!
//! The 647-entry bigram table is ported verbatim from oh-my-pi
//! (`packages/coding-agent/src/hashline/bigrams.json`, MIT). Each entry is
//! a 2-letter lowercase pair that tokenizes as exactly one BPE token in
//! every modern vocabulary (cl100k / o200k / Claude family), so the
//! complete anchor `42sr` costs two tokens (`42` + `sr`). The 29 missing
//! pairs are rare-letter combinations (mostly q/x/z) that no mainstream
//! vocabulary merges. Hash space = 647 ≈ 9.3 bits.
//!
//! ## Why this module is pure
//!
//! No file I/O, no environment, no clock. All inputs are `&str`/`&[…]`.
//! Tests live next to the code and exercise the public interface only.
//!
//! ## Module layout
//!
//! - [`anchor`] — the hash alphabet, `compute_line_hash`, the
//!   `LINEhh|content` formatters, and anchor parsing/rendering.
//! - [`apply`] — the edit operations and the splice engine that validates
//!   anchors and applies a batch of edits.

mod anchor;
mod apply;

pub use anchor::{
    Anchor, AnchorOrBoundary, BODY_SEP, HASH_WIDTH, InsertSide, compute_line_hash,
    format_hash_line, format_hash_lines,
};
pub use apply::{Applied, ApplyError, HashEdit, StaleAnchor, apply_hash_edits};

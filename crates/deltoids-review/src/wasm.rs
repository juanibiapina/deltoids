//! C-ABI for the browser reviewer (`reviewer/src/core/reviewCore.ts`).
//!
//! Strings and arrays cross linear memory as `(ptr, len)` pairs from
//! [`alloc`]. Results come back as a packed `ptr << 32 | len` handle that
//! the caller frees with [`dealloc`].

use std::alloc::Layout;

use crate::overview::{Change, Span, change_cells as cells_for};
use crate::tree::{TreeRow, rows};

fn layout(len: usize) -> Layout {
    Layout::from_size_align(len, 8).expect("byte layout is always valid")
}

#[unsafe(no_mangle)]
pub extern "C" fn alloc(len: usize) -> *mut u8 {
    if len == 0 {
        return std::ptr::NonNull::<u64>::dangling().as_ptr().cast();
    }
    unsafe { std::alloc::alloc(layout(len)) }
}

/// # Safety
/// `ptr`/`len` must come from [`alloc`] or a result of this module.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dealloc(ptr: *mut u8, len: usize) {
    if !ptr.is_null() && len != 0 {
        unsafe { std::alloc::dealloc(ptr, layout(len)) }
    }
}

fn hand_back(bytes: Vec<u8>) -> u64 {
    let len = bytes.len();
    let ptr = alloc(len);
    if len != 0 {
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, len) };
    }
    ((ptr as u64) << 32) | len as u64
}

fn push_json_string(out: &mut String, text: &str) {
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Tree rows for NUL-separated paths, indexed by position, as JSON:
/// `[[0, depth, label], [1, depth, index, name], ...]`.
///
/// # Safety
/// `paths`/`len` must point at `len` readable bytes of UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tree_rows(paths: *const u8, len: usize) -> u64 {
    let bytes = if len == 0 {
        &[][..]
    } else {
        unsafe { std::slice::from_raw_parts(paths, len) }
    };
    let text = String::from_utf8_lossy(bytes);
    let mut json = String::from("[");
    let entries = if text.is_empty() {
        None
    } else {
        Some(text.split('\0'))
    };
    for (n, row) in rows(entries.into_iter().flatten().enumerate())
        .iter()
        .enumerate()
    {
        if n > 0 {
            json.push(',');
        }
        match row {
            TreeRow::Dir { label, depth } => {
                json.push_str(&format!("[0,{depth},"));
                push_json_string(&mut json, label);
            }
            TreeRow::File { index, name, depth } => {
                json.push_str(&format!("[1,{depth},{index},"));
                push_json_string(&mut json, name);
            }
        }
        json.push(']');
    }
    json.push(']');
    hand_back(json.into_bytes())
}

/// Overview cells for `count` spans stored as `f64` triples
/// `(top, height, kind)`, kind 0 for added and 1 for removed. Returns one
/// byte per cell: bit 0 added, bit 1 removed.
///
/// # Safety
/// `spans` must point at `count * 3` readable, aligned `f64`s.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn change_cells(
    spans: *const f64,
    count: usize,
    content: f64,
    track: f64,
    cell: f64,
) -> u64 {
    let raw = if count == 0 {
        &[][..]
    } else {
        unsafe { std::slice::from_raw_parts(spans, count * 3) }
    };
    let spans: Vec<Span> = raw
        .chunks_exact(3)
        .map(|s| Span {
            top: s[0],
            height: s[1],
            change: if s[2] == 0.0 {
                Change::Added
            } else {
                Change::Removed
            },
        })
        .collect();
    let bytes = cells_for(&spans, content, track, cell)
        .into_iter()
        .map(|c| u8::from(c.added) | (u8::from(c.removed) << 1))
        .collect();
    hand_back(bytes)
}

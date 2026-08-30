//! WebAssembly bindings for the deltoids diff engine.
//!
//! The browser PR reviewer calls these C-ABI exports directly over the
//! module's linear memory (no wasm-bindgen, so the same module runs under a
//! plain WASI shim). The flow is:
//!
//! 1. `alloc(len)` reserves a buffer; JS copies a UTF-8 string into it.
//! 2. `render_file(before*, before_len, after*, after_len, path*, path_len)`
//!    computes the diff and returns a packed `ptr:len` handle to the HTML
//!    bytes (`ptr << 32 | len`).
//! 3. JS reads the HTML from memory, then frees both the inputs and the
//!    result with `dealloc(ptr, len)`.
//!
//! All buffers are plain `Vec<u8>` whose ownership is handed across the FFI
//! boundary and returned for freeing, so there are no leaks as long as the
//! caller pairs every `alloc`/result with a `dealloc`.

use std::alloc::Layout;

use deltoids::Diff;
use deltoids::parse::GitDiff;
use deltoids::render_html::render_entry_html;
use deltoids::reverse::reconstruct_before;

/// Layout for a raw byte buffer of `len` bytes (align 1). Allocation and
/// freeing must use the identical layout, so both go through here.
fn layout(len: usize) -> Layout {
    Layout::from_size_align(len, 1).expect("byte layout is always valid")
}

/// Reserve `len` bytes and return a pointer the caller can write into.
///
/// Callers must pair every `alloc` with a matching `dealloc` using the same
/// `len`. A zero-length request returns a non-null dangling pointer.
#[unsafe(no_mangle)]
pub extern "C" fn alloc(len: usize) -> *mut u8 {
    if len == 0 {
        return std::ptr::NonNull::<u8>::dangling().as_ptr();
    }
    // Safety: layout has non-zero size here.
    unsafe { std::alloc::alloc(layout(len)) }
}

/// Free a buffer previously returned by [`alloc`] or a `render_*` result.
/// `len` must be the exact length used to allocate it.
///
/// # Safety
/// `ptr`/`len` must originate from this module's `alloc`/`render_*`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dealloc(ptr: *mut u8, len: usize) {
    if ptr.is_null() || len == 0 {
        return;
    }
    unsafe { std::alloc::dealloc(ptr, layout(len)) }
}

/// Render the deltoids diff HTML body from full before/after content.
///
/// This is the safe core behind [`render_file`]; the FFI wrapper only
/// marshals strings across linear memory.
pub fn render_html(before: &str, after: &str, path: &str) -> String {
    let diff = Diff::compute(before, after, path);
    render_entry_html(diff.hunks(), diff.highlight())
}

/// Render the diff HTML from `after` content plus a unified `patch` (GitHub's
/// per-file `patch` field), reconstructing the before side by reverse-applying
/// the patch. The `patch` is just the `@@` hunks, so a dummy `diff --git`
/// header is synthesized for the parser; reconstruction ignores the paths.
pub fn render_html_from_patch(after: &str, patch: &str, path: &str) -> String {
    let synthetic = format!("diff --git a/f b/f\n--- a/f\n+++ b/f\n{patch}");
    let before = match GitDiff::parse(&synthetic).files.first() {
        Some(file) => reconstruct_before(after, file),
        None => after.to_string(),
    };
    render_html(&before, after, path)
}

/// Compute the deltoids diff between `before` and `after` for `path` and
/// return the rendered HTML body as a packed `ptr << 32 | len` handle.
///
/// # Safety
/// The three `(ptr, len)` pairs must describe valid UTF-8 buffers owned by
/// this module's linear memory (typically from [`alloc`]).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn render_file(
    before_ptr: *const u8,
    before_len: usize,
    after_ptr: *const u8,
    after_len: usize,
    path_ptr: *const u8,
    path_len: usize,
) -> u64 {
    let before = unsafe { str_from_parts(before_ptr, before_len) };
    let after = unsafe { str_from_parts(after_ptr, after_len) };
    let path = unsafe { str_from_parts(path_ptr, path_len) };
    pack_bytes(render_html(&before, &after, &path).into_bytes())
}

/// Like [`render_file`], but reconstructs the `before` content from a unified
/// `patch` so the client can fetch only the head-side content.
///
/// # Safety
/// The `(ptr, len)` pairs must describe valid buffers owned by this module's
/// linear memory (typically from [`alloc`]).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn render_from_patch(
    after_ptr: *const u8,
    after_len: usize,
    patch_ptr: *const u8,
    patch_len: usize,
    path_ptr: *const u8,
    path_len: usize,
) -> u64 {
    let after = unsafe { str_from_parts(after_ptr, after_len) };
    let patch = unsafe { str_from_parts(patch_ptr, patch_len) };
    let path = unsafe { str_from_parts(path_ptr, path_len) };
    pack_bytes(render_html_from_patch(&after, &patch, &path).into_bytes())
}

/// Copy `len` bytes at `ptr` into an owned `String`, lossily decoding any
/// invalid UTF-8 so a malformed blob cannot panic the module.
unsafe fn str_from_parts(ptr: *const u8, len: usize) -> String {
    if ptr.is_null() || len == 0 {
        return String::new();
    }
    let bytes = unsafe { std::slice::from_raw_parts(ptr, len) };
    String::from_utf8_lossy(bytes).into_owned()
}

/// Copy `bytes` into a freshly `alloc`ated buffer and pack its pointer and
/// length into a single `u64` (`ptr << 32 | len`) for return across the FFI
/// boundary. Using `alloc` keeps the free path symmetric with `dealloc`.
/// wasm32 pointers and lengths both fit in 32 bits.
fn pack_bytes(bytes: Vec<u8>) -> u64 {
    let len = bytes.len();
    let ptr = alloc(len);
    if len > 0 {
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, len) };
    }
    ((ptr as u64) << 32) | (len as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BEFORE: &str = "fn main() {\n    println!(\"hi\");\n}\n";
    const AFTER: &str = "fn main() {\n    println!(\"bye\");\n    let x = 1;\n}\n";
    const PATCH: &str = "@@ -1,3 +1,4 @@\n fn main() {\n-    println!(\"hi\");\n+    println!(\"bye\");\n+    let x = 1;\n }";

    #[test]
    fn render_html_emits_diff_rows_and_scope() {
        let html = render_html(BEFORE, AFTER, "src/main.rs");
        assert!(html.contains("class=\"row added\""));
        assert!(html.contains("class=\"row removed\""));
        assert!(html.contains("class=\"breadcrumb\""));
        assert!(html.contains("main"));
    }

    #[test]
    fn render_from_patch_matches_full_render() {
        let full = render_html(BEFORE, AFTER, "src/main.rs");
        let from_patch = render_html_from_patch(AFTER, PATCH, "src/main.rs");
        assert_eq!(full, from_patch);
    }

    // The FFI packs `ptr << 32 | len`, which is only valid for 32-bit wasm
    // pointers, so exercise only the allocator roundtrip on the host.
    #[test]
    fn alloc_dealloc_roundtrip_is_writable() {
        let ptr = alloc(8);
        unsafe {
            std::ptr::write_bytes(ptr, 0xAB, 8);
            assert_eq!(*ptr, 0xAB);
            dealloc(ptr, 8);
        }
    }
}

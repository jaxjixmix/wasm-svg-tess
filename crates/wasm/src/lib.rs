//! Raw C-ABI surface over the tessellator.
//!
//! Two-call convention for variable-length output, same as the Markdown repo:
//!
//! ```text
//! tessellate(path_ptr, path_len) -> u32   // triangle count, fills an internal buffer
//! result_ptr() -> *const f32             // 6 floats per triangle (x,y x3)
//! ```
//!
//! Output is `f32` rather than `f64` because renderers consume `f32` and halving
//! the transfer halved the boundary cost in practice. The internal maths stays
//! `f64`; only the hand-off narrows.

use std::alloc::{alloc as raw_alloc, dealloc as raw_dealloc, Layout};
use std::cell::RefCell;

const ALIGN: usize = 4;

thread_local! {
    /// Last tessellation result, as f32 `x,y` pairs, 6 per triangle.
    static RESULT: RefCell<Vec<f32>> = const { RefCell::new(Vec::new()) };
    /// Vertex count of the flattened polygon, for diagnostics.
    static LAST_VERTS: RefCell<u32> = const { RefCell::new(0) };
}

/// Allocate `len` bytes. Returns null on failure.
///
/// # Safety
/// Caller owns the block and must pass the same `len` to `dealloc`.
#[no_mangle]
pub extern "C" fn alloc(len: usize) -> *mut u8 {
    if len == 0 {
        return std::ptr::null_mut();
    }
    match Layout::from_size_align(len, ALIGN) {
        Ok(l) => unsafe { raw_alloc(l) },
        Err(_) => std::ptr::null_mut(),
    }
}

/// Free a block from `alloc`.
///
/// # Safety
/// `ptr` must come from `alloc(len)` and must not be used afterwards.
#[no_mangle]
pub unsafe extern "C" fn dealloc(ptr: *mut u8, len: usize) {
    if ptr.is_null() || len == 0 {
        return;
    }
    if let Ok(l) = Layout::from_size_align(len, ALIGN) {
        raw_dealloc(ptr, l);
    }
}

/// Tessellate an SVG path `d` string. Returns the triangle count.
///
/// # Safety
/// `ptr` must be valid for `len` bytes of UTF-8.
#[no_mangle]
pub unsafe extern "C" fn tessellate(ptr: *const u8, len: usize) -> u32 {
    let input = if len == 0 {
        ""
    } else {
        std::str::from_utf8(std::slice::from_raw_parts(ptr, len)).unwrap_or_default()
    };

    let tris = tess::tessellate(input);
    // tessellate() returns 6 floats per triangle.
    let tri_count = (tris.len() / 6) as u32;

    RESULT.with(|r| {
        let mut buf = r.borrow_mut();
        buf.clear();
        // Narrow f64 -> f32 here, once, at the boundary.
        buf.extend(tris.iter().map(|v| *v as f32));
    });

    LAST_VERTS.with(|v| *v.borrow_mut() = tri_count * 3);
    tri_count
}

/// Pointer to the buffer filled by the last `tessellate` call.
#[no_mangle]
pub extern "C" fn result_ptr() -> *const f32 {
    RESULT.with(|r| r.borrow().as_ptr())
}

/// Flatten only, no triangulation. Returns the vertex count of the first
/// subpath and points `result_ptr` at `x,y` pairs.
///
/// Exposed separately because flattening is the part whose accuracy can be
/// proven against a sampled curve, so the verifier wants it without triangle
/// indices in the way.
///
/// # Safety
/// `ptr` must be valid for `len` bytes.
#[no_mangle]
pub unsafe extern "C" fn flatten_path(ptr: *const u8, len: usize) -> u32 {
    let input = if len == 0 {
        ""
    } else {
        std::str::from_utf8(std::slice::from_raw_parts(ptr, len)).unwrap_or_default()
    };

    let cmds = tess::parse_path(input);
    let polys = tess::flatten(&cmds);
    let first = polys.first().cloned().unwrap_or_default();

    RESULT.with(|r| {
        let mut buf = r.borrow_mut();
        buf.clear();
        for p in &first {
            buf.push(p.x as f32);
            buf.push(p.y as f32);
        }
    });

    (first.len() * 2) as u32
}

/// ABI canary so the harness can detect a stale build.
#[no_mangle]
pub extern "C" fn abi_version() -> u32 {
    1
}

/// Flattening tolerance, so the verifier can assert against the same value the
/// implementation used rather than duplicating a constant.
#[no_mangle]
pub extern "C" fn tolerance() -> f64 {
    tess::TOLERANCE
}

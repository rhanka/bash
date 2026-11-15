//! The binding module: the generated canonical-ABI glue (`kernel.rs`, committed, regenerated only when the
//! WIT changes, see `tools/build.mjs --check-bindings`), its small runtime (`rt`) and the adapter that
//! implements the generated traits on top of the kernel proper (`adapt`). Compiled for wasm only: this is
//! the one place besides `mem` where `unsafe` and the `alloc` heap types are allowed.
#[allow(unexpected_cfgs, unused, unsafe_op_in_unsafe_fn, clippy::all, static_mut_refs)]
pub mod kernel;
mod adapt;
pub mod rt;

/// Allocation hook of the canonical ABI: every lowered value is allocated through `kernel::mem`, which
/// traps rather than returning null (EVOL D6).
#[unsafe(export_name = "cabi_realloc")]
pub unsafe extern "C" fn cabi_realloc(old_ptr: *mut u8, old_len: usize, align: usize, new_len: usize) -> *mut u8 {
    // SAFETY: the canonical ABI contract is exactly that of `mem::cabi_realloc`.
    unsafe { crate::mem::cabi_realloc(old_ptr, old_len, align, new_len) }
}

/// `memcmp` for the core library: the wasm target links no C library, and `core` calls this for slice
/// comparisons. A plain byte loop; `black_box` keeps the optimizer from turning it back into a call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32 {
    let mut i = 0;
    while i < n {
        // SAFETY: the caller guarantees both pointers are valid for `n` bytes.
        let (x, y) = unsafe { (*a.add(i), *b.add(i)) };
        if x != y {
            return core::hint::black_box(x as i32 - y as i32);
        }
        i += 1;
    }
    0
}

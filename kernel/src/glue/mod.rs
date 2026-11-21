//! The glue module: the generated canonical-ABI bindings (`kernel/src/bindings/kernel.rs`, committed and
//! regenerated only when the WIT changes, see `tools/build.mjs --check-bindings`; that directory holds the
//! generated file and nothing else), the small runtime they need (`rt`) and the adapter that implements the
//! generated traits on top of the kernel proper (`adapt`). Compiled for wasm only. Besides `mem`, this is
//! where `unsafe` is allowed; its heap types come from `mem::abi`.
#[allow(unexpected_cfgs, unused, unsafe_op_in_unsafe_fn, clippy::all, static_mut_refs)]
#[path = "../bindings/kernel.rs"]
pub mod kernel;
mod adapt;
pub mod rt;

/// Allocation hook of the canonical ABI: every lowered value is allocated through `kernel::mem`, which
/// traps rather than returning null (EVOL D6).
///
/// # Safety
/// Called by the canonical ABI with its `realloc` contract, see `mem::cabi_realloc`.
#[unsafe(export_name = "cabi_realloc")]
pub unsafe extern "C" fn cabi_realloc(old_ptr: *mut u8, old_len: usize, align: usize, new_len: usize) -> *mut u8 {
    // SAFETY: the canonical ABI contract is exactly that of `mem::cabi_realloc`.
    unsafe { crate::mem::cabi_realloc(old_ptr, old_len, align, new_len) }
}

/// `memcmp` for the core library: the wasm target links no C library, and `core` calls this for slice
/// comparisons. A plain byte loop; `black_box` keeps the optimizer from turning it back into a call.
///
/// # Safety
/// `a` and `b` must each be valid for reads of `n` bytes.
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

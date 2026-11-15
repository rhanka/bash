//! Transfer hook of the canonical ABI (spec 5.8). A function only in this lot: not exported from the wasm
//! module, and no headroom accounting (L1a-2 adds it here and in `transfer_free`).
use alloc::alloc::{alloc, dealloc, realloc, Layout};

fn trap() -> ! {
    #[cfg(target_arch = "wasm32")]
    core::arch::wasm32::unreachable();
    #[cfg(not(target_arch = "wasm32"))]
    panic!("cabi_realloc: the allocator refused");
}

/// Allocation hook of the canonical ABI, for the binding module.
///
/// `new_len == 0` returns a non-null pointer equal to `align` and allocates nothing (a previous block, if
/// any, is freed); `old_len == 0` allocates `new_len` bytes with alignment `align`; otherwise the block
/// (`old_ptr`, `old_len`, `align`) is reallocated to `new_len`, preserving the first `min(old_len,
/// new_len)` bytes. It never returns null: when the allocator refuses (ceiling, or an invalid layout) it
/// traps (`unreachable` on wasm, panic natively).
///
/// # Safety
/// Arguments follow the canonical ABI contract; `align` is a power of two.
pub unsafe fn cabi_realloc(old_ptr: *mut u8, old_len: usize, align: usize, new_len: usize) -> *mut u8 {
    if new_len == 0 {
        if old_len != 0 {
            // SAFETY: the caller passes a live block from this hook with these parameters.
            unsafe { transfer_free(old_ptr, old_len, align) };
        }
        return align as *mut u8;
    }
    let Ok(new_layout) = Layout::from_size_align(new_len, align) else { trap() };
    let raw = if old_len == 0 {
        // SAFETY: `new_layout` has a non-zero size.
        unsafe { alloc(new_layout) }
    } else {
        // SAFETY: the caller passes a live block from this hook allocated with (`old_len`, `align`);
        // `new_len` is non-zero and its layout was validated above.
        unsafe { realloc(old_ptr, Layout::from_size_align_unchecked(old_len, align), new_len) }
    };
    if raw.is_null() {
        trap();
    }
    raw
}

/// Releases a block obtained from `cabi_realloc`. A zero length is a no-op.
///
/// # Safety
/// (`ptr`, `len`, `align`) designate a live block from `cabi_realloc`.
pub unsafe fn transfer_free(ptr: *mut u8, len: usize, align: usize) {
    if len == 0 {
        return;
    }
    // SAFETY: the caller guarantees the block was allocated with exactly this size and alignment.
    unsafe { dealloc(ptr, Layout::from_size_align_unchecked(len, align)) };
}

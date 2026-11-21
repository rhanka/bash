//! Gate 10: with the ceiling exhausted the native build of the hook panics.
mod common;
use common::*;
use sentropic_bash_kernel::mem::*;

#[test]
#[should_panic(expected = "cabi_realloc")]
fn cabi_realloc_traps_when_the_ceiling_is_exhausted() {
    let used = start();
    configure(used + 16).unwrap();
    // SAFETY: a fresh allocation request; the allocator refuses and the hook traps.
    let _ = unsafe { cabi_realloc(core::ptr::null_mut(), 0, 8, 4096) };
}

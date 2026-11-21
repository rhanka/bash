//! Gate 8 of lot L1a-2: exceeding the transfer account panics natively (it traps on wasm).
mod common;
use common::*;
use sentropic_bash_kernel::mem::*;

#[test]
#[should_panic(expected = "cabi_realloc")]
fn cabi_realloc_traps_when_the_account_is_exceeded() {
    start();
    set_headroom(1000).unwrap();
    // SAFETY: a fresh request; the account refuses it and the hook traps.
    let _ = unsafe { cabi_realloc(core::ptr::null_mut(), 0, 8, 1001) };
}

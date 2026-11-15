//! Kernel of @sentropic/bash. Empty in L0; later lots add the behaviour.
#![cfg_attr(target_arch = "wasm32", no_std)]

#[cfg(all(target_arch = "wasm32", not(test)))]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    core::arch::wasm32::unreachable()
}

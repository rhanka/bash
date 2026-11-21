//! Kernel of @sentropic/bash. The kernel exports nothing yet; later lots add the behaviour.
//! Every heap allocation of the kernel goes through [`mem`].
#![cfg_attr(target_arch = "wasm32", no_std)]
#![deny(unsafe_op_in_unsafe_fn)]

pub mod mem;

#[cfg(all(target_arch = "wasm32", not(test)))]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    core::arch::wasm32::unreachable()
}

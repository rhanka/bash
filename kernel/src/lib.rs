//! Kernel of @sentropic/bash. It exports the engine contract of EVOL Annex A: the resources `vfs`,
//! `shell` and `fiber`, run by a bootstrap interpreter until the shell language (L2) replaces it.
//! Every heap allocation of the kernel goes through [`mem`]; the generated glue in [`bindings`] is the
//! one exception, and it too allocates through the global allocator that [`mem`] installs.
#![cfg_attr(target_arch = "wasm32", no_std)]
#![deny(unsafe_op_in_unsafe_fn)]

pub mod fiber;
pub mod fs;
pub mod limits;
pub mod mem;
pub mod shell;
pub mod util;

#[cfg(target_arch = "wasm32")]
mod bindings;

#[cfg(all(target_arch = "wasm32", not(test)))]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    core::arch::wasm32::unreachable()
}

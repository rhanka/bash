//! Kernel of @sentropic/bash. It exports the engine contract of EVOL Annex A: the resources `vfs`,
//! `shell` and `fiber`, with a resumable shell virtual machine.
//! Every heap allocation of the kernel goes through [`mem`]; the glue in [`glue`] only names the
//! heap types of the generated traits (through `mem::abi`), and they too allocate through the global
//! allocator that [`mem`] installs.
#![cfg_attr(target_arch = "wasm32", no_std)]
#![deny(unsafe_op_in_unsafe_fn)]

pub mod base64;
pub mod cmd;
pub mod fiber;
pub mod format;
pub mod fs;
pub mod hash;
pub mod limits;
pub mod mem;
pub mod num;
pub mod shell;
pub mod util;

// The standalone allocator probe is instantiated as a core module without resource imports.
#[cfg(all(target_arch = "wasm32", not(feature = "mem-test-exports")))]
mod glue;

#[cfg(all(target_arch = "wasm32", not(test)))]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    core::arch::wasm32::unreachable()
}

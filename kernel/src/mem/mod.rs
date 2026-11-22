//! The memory module of the kernel: the one door to the heap (spec L1a-1).
//!
//! * Errors: [`MemError`], [`LimitId`], [`MemResult`].
//! * Instance ceiling: [`configure`], [`raise_ceiling`], [`usage`] on top of a global allocator wrapper.
//! * Budgets: [`Budget`], a nestable logical byte cap.
//! * Containers: [`KVec`], [`KBytes`], [`KString`], [`KBox`], [`KRc`], [`KMap`] (a B-tree), [`KInsertMap`]
//!   (insertion-ordered) and [`KChunks`] (chunked bytes), all fallible, with [`TryClone`]. Nothing here grows
//!   infallibly.
//! * Accounting: [`footprint`], [`headroom`], [`reserve_scope`].
//! * Transfer hook: [`cabi_realloc`], [`transfer_free`].
//!
//! The canonical-ABI heap types are internal to the Wasm bindings.
//!
//! ```compile_fail
//! use sentropic_bash_kernel::mem::abi::Vec;
//! ```
//!
//! The module is single-threaded by design: its types are neither `Send` nor `Sync`.
// The alloc crate is declared here, inside kernel/src/mem/, and reached by the submodules as `super::alloc`.
extern crate alloc;

/// The heap types the generated canonical-ABI traits are written in (`Vec`, `String`, the allocator
/// entry points). Only `kernel/src/glue/` reaches them, through this one door; everything else uses the
/// fallible containers of this module.
#[cfg(all(target_arch = "wasm32", not(feature = "mem-test-exports")))]
pub(crate) mod abi {
    pub use super::alloc::alloc::{alloc, dealloc, Layout};
    pub use super::alloc::string::String;
    pub use super::alloc::vec;
    pub use super::alloc::vec::Vec;

    /// The binding export of `mem::cabi_realloc`. Keep its transfer charge attached to the block
    /// because generated bindings can adopt it into a `Vec` or free it directly after a lift.
    ///
    /// # Safety
    /// The arguments must satisfy `mem::cabi_realloc`'s contract.
    pub(crate) unsafe fn cabi_realloc(old: *mut u8, old_len: usize, align: usize, new_len: usize) -> *mut u8 {
        use super::heap::WasmHeap;
        if old_len != 0 {
            // SAFETY: `old` is a live transfer block. The transfer hook accounts for its resize or free.
            unsafe { WasmHeap::mark_transfer(old, false) };
        }
        // SAFETY: the arguments have the canonical ABI contract, forwarded unchanged.
        let ptr = unsafe { super::cabi_realloc(old, old_len, align, new_len) };
        if new_len != 0 {
            // SAFETY: the hook just allocated or resized this nonempty block and charged its size.
            unsafe { WasmHeap::mark_transfer(ptr, true) };
        }
        ptr
    }
}

mod boxed;
mod budget;
mod ceiling;
mod chunks;
mod clone;
mod error;
#[cfg(any(target_arch = "wasm32", test))]
mod heap;
mod headroom;
mod insertmap;
mod map;
mod rc;
mod reserve;
mod string;
#[cfg(all(feature = "mem-test-exports", target_arch = "wasm32"))]
mod testexports;
mod transfer;
mod vec;

pub use boxed::KBox;
pub use budget::Budget;
pub use chunks::KChunks;
pub use ceiling::{configure, raise_ceiling, usage, ConfigError, Usage};
#[cfg(target_arch = "wasm32")]
pub(crate) use ceiling::is_pristine;
pub use clone::TryClone;
pub use error::{LimitId, MemError, MemResult};
pub use headroom::{headroom, set_headroom};
pub use insertmap::KInsertMap;
pub use map::KMap;
pub use rc::KRc;
pub use reserve::reserve_scope;
#[cfg(target_arch = "wasm32")]
pub(crate) use reserve::is_open;
pub use string::KString;
pub use transfer::{cabi_realloc, transfer_free};
pub use vec::{KBytes, KVec};

#[cfg(target_arch = "wasm32")]
type Inner = heap::WasmHeap;
#[cfg(not(target_arch = "wasm32"))]
type Inner = std::alloc::System;

#[cfg(target_arch = "wasm32")]
const fn inner() -> Inner {
    heap::WasmHeap::new()
}
#[cfg(not(target_arch = "wasm32"))]
const fn inner() -> Inner {
    std::alloc::System
}

#[global_allocator]
static ALLOCATOR: ceiling::Ceiling<Inner> = ceiling::Ceiling(inner());

/// Test support, native target only: process-wide state cannot be reset by the public interface.
#[cfg(not(target_arch = "wasm32"))]
#[doc(hidden)]
pub mod testing {
    /// Forgets the configuration (so `configure` is valid again) and sets the ceiling to `ceiling_bytes`.
    /// Does not touch the bytes in use; `high_water` restarts from the current `used`.
    pub fn reset(ceiling_bytes: u64) {
        super::ceiling::reset(ceiling_bytes);
    }
}

/// Grows linear memory by exactly `pages` for a snapshot restore, through the allocator that
/// owns `memory.grow`. The restored image carries the heap metadata itself, so no free block is
/// inserted. Fails (growing nothing) when the final size would exceed the explicit 268,435,456 B
/// default cap or the engine refuses the growth. Restore-only: ordinary allocations never call it.
#[cfg(target_arch = "wasm32")]
pub(crate) fn restore_grow(pages: usize) -> bool {
    ALLOCATOR.0.restore_grow(pages)
}

/// Bytes of memory managed by the allocator (on the native target: equal to `usage().used`).
pub fn footprint() -> u64 {
    #[cfg(target_arch = "wasm32")]
    {
        ALLOCATOR.0.footprint()
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        usage().used
    }
}

/// Highest footprint since start (on the native target: `usage().high_water`).
pub fn footprint_high_water() -> u64 {
    #[cfg(target_arch = "wasm32")]
    {
        ALLOCATOR.0.footprint_high_water()
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        usage().high_water
    }
}

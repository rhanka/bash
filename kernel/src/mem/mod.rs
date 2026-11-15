//! The memory module of the kernel: the one door to the heap (spec L1a-1).
//!
//! * Errors: [`MemError`], [`LimitId`], [`MemResult`].
//! * Instance ceiling: [`configure`], [`raise_ceiling`], [`usage`] on top of a global allocator wrapper.
//! * Budgets: [`Budget`], a nestable logical byte cap.
//! * Containers: [`KVec`], [`KBytes`], [`KString`], [`KBox`], [`KRc`], [`KMap`], all fallible, with
//!   [`TryClone`]. Nothing here grows infallibly.
//! * Transfer hook: [`cabi_realloc`], [`transfer_free`].
//!
//! The module is single-threaded by design: its types are neither `Send` nor `Sync`.
// The alloc crate is declared here, inside kernel/src/mem/, and reached by the submodules as `super::alloc`.
extern crate alloc;

mod boxed;
mod budget;
mod ceiling;
mod clone;
mod error;
#[cfg(any(target_arch = "wasm32", test))]
mod heap;
mod headroom;
mod map;
mod rc;
mod reserve;
mod string;
mod transfer;
mod vec;

pub use boxed::KBox;
pub use budget::Budget;
pub use ceiling::{configure, raise_ceiling, usage, ConfigError, Usage};
pub use clone::TryClone;
pub use error::{LimitId, MemError, MemResult};
pub use headroom::{headroom, set_headroom};
pub use map::KMap;
pub use rc::KRc;
pub use reserve::reserve_scope;
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

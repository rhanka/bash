//! Snapshot restore preflight, growth and capture probe (spec L2c section 4).
//!
//! The Wasm-owned default image is installed by a tool-generated `__restore_default` export that
//! calls the two helpers below by index. They are invisible to the WIT: ordinary guests never call
//! them, and the component's world is unchanged. `__snapshot_probe` is read-only support for the
//! offline capture tool; it never changes state.
use crate::mem;

/// Preflight of `__restore_default`: 1 when this store is fresh enough to restore, else 0.
/// Requires no configuration yet, no allocated byte, a pristine high-water and exactly
/// `expected_pages` of linear memory. Never changes state: a failed preflight leaves memory,
/// handles and any latch untouched.
///
/// # Safety
/// Exported with a stable Wasm name for the tool-generated restore; the signature is plain data.
#[unsafe(export_name = "__restore_preflight")]
pub extern "C" fn restore_preflight(expected_pages: u32) -> u32 {
    let pages = core::arch::wasm32::memory_size(0);
    u32::from(mem::is_pristine() && pages == expected_pages as usize)
}

/// Allocator-owned page growth of `__restore_default`: 1 after growing exactly `pages`, else 0.
/// The final linear-memory size is checked against the explicit 268,435,456 B default cap; on
/// refusal nothing is grown, written or registered. The heap metadata comes from the image
/// itself, so no free block is inserted.
///
/// # Safety
/// Exported with a stable Wasm name for the tool-generated restore; the signature is plain data.
#[unsafe(export_name = "__restore_grow")]
pub extern "C" fn restore_grow(pages: u32) -> u32 {
    u32::from(mem::restore_grow(pages as usize))
}

/// Read-only capture probe for the offline snapshot tool: bit 0 is set when the transfer headroom
/// account is empty, bit 1 when the allocator reserve is closed, bit 2 when no lowering slot is
/// held. A capture requires all three.
///
/// # Safety
/// Exported with a stable Wasm name for the offline capture tool; the signature is plain data.
#[unsafe(export_name = "__snapshot_probe")]
pub extern "C" fn snapshot_probe() -> u64 {
    let mut probe = 0u64;
    if mem::headroom().used() == 0 {
        probe |= 1;
    }
    if !mem::is_open() {
        probe |= 1 << 1;
    }
    if super::rt::lowering_clean() {
        probe |= 1 << 2;
    }
    probe
}

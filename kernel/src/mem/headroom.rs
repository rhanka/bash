//! The transfer account (spec L1a-2 5.1): a budget with limit [`LimitId::HEADROOM`] that `cabi_realloc` charges
//! for the canonical-ABI buffers it hands out and `transfer_free` releases. Its node lives in a static, so
//! the account needs no allocation and cannot fail to exist.
use super::budget::{Budget, StaticNode};
use super::ceiling::{ConfigError, BOOTSTRAP_CEILING};
use super::error::LimitId;
use core::sync::atomic::{AtomicBool, Ordering::Relaxed};

/// Default cap: this fraction of the ceiling (one eighth).
const FRACTION: u64 = 8;

static ACCOUNT: StaticNode = StaticNode::new(LimitId::HEADROOM, BOOTSTRAP_CEILING / FRACTION);
static EXPLICIT: AtomicBool = AtomicBool::new(false);

/// The transfer account: limit id [`LimitId::HEADROOM`]. Cloning the handle never fails.
pub fn headroom() -> Budget {
    Budget::of_static(&ACCOUNT)
}

/// Sets the cap of the transfer account (canonical-ABI buffers). Until this function is called the cap is
/// one eighth of the ceiling, recomputed whenever the ceiling changes. Fails with `BelowUsage`, changing
/// nothing, when `bytes` is below the bytes currently charged to the account.
pub fn set_headroom(bytes: u64) -> Result<(), ConfigError> {
    let account = headroom();
    if bytes < account.used() {
        return Err(ConfigError::BelowUsage);
    }
    account.set_cap(bytes);
    EXPLICIT.store(true, Relaxed);
    Ok(())
}

/// Called by the ceiling when it changes: recomputes the default cap unless `set_headroom` was called.
/// Never lowers the cap below the current use.
pub(crate) fn ceiling_changed(ceiling: u64) {
    if EXPLICIT.load(Relaxed) {
        return;
    }
    let account = headroom();
    account.set_cap((ceiling / FRACTION).max(account.used()));
}

/// Test support: forgets an explicit cap.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn forget_explicit() {
    EXPLICIT.store(false, Relaxed);
}

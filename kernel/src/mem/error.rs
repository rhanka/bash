//! Errors of the memory module (spec 5.1).

/// Identifies a named limit of EVOL Annex B. 0 and 1 are reserved here; later lots assign the others.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct LimitId(pub u16);

impl LimitId {
    /// Instance memory ceiling.
    pub const CEILING: LimitId = LimitId(0);
    /// Transfer buffers (accounting completed in L1a-2).
    pub const HEADROOM: LimitId = LimitId(1);
}

/// Why a fallible memory operation failed. A failed operation leaves its container unchanged.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MemError {
    /// A budget refused the charge. Carries the limit of the budget that refused
    /// (the nearest refusing ancestor when budgets are nested).
    Limit(LimitId),
    /// The allocator refused: the instance ceiling would be exceeded.
    Ceiling,
    /// A size computation overflowed `usize` (for example capacity * element size).
    Overflow,
}

/// Result of a fallible memory operation.
pub type MemResult<T> = core::result::Result<T, MemError>;

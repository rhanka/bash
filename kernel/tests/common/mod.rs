//! Shared helpers. The ceiling and the counters are process-wide, so each test binary holds exactly one
//! `#[test]` function that runs its scenarios in order: nothing else allocates while it measures.
#![allow(dead_code)]
use sentropic_bash_kernel::mem::{testing, usage, Budget, LimitId};

pub const BIG: u64 = 1 << 30;

/// Gives the test a roomy ceiling and returns the bytes in use at the start.
pub fn start() -> u64 {
    testing::reset(BIG);
    usage().used
}

pub fn root(id: u16, cap: u64) -> Budget {
    Budget::root(LimitId(id), cap).unwrap()
}

/// Fixed-seed xorshift generator.
pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    pub fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

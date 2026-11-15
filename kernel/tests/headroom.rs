//! Gate 8 of lot L1a-2 (native part): the transfer account behind `cabi_realloc` and `transfer_free`.
mod common;
use common::*;
use sentropic_bash_kernel::mem::*;

#[test]
fn headroom_account() {
    let base = start();
    let h = headroom();
    assert_eq!(h.limit(), LimitId::HEADROOM);
    assert_eq!(h.used(), 0);
    assert_eq!(h.cap(), BIG / 8, "default cap: one eighth of the ceiling");

    // The default follows the ceiling.
    testing::reset(1 << 20);
    assert_eq!(headroom().cap(), (1 << 20) / 8);
    configure(1 << 24).unwrap();
    assert_eq!(headroom().cap(), (1 << 24) / 8);
    raise_ceiling(1 << 25).unwrap();
    assert_eq!(headroom().cap(), (1 << 25) / 8);

    // cabi_realloc charges what it hands out; transfer_free and shrinking release it.
    // SAFETY: calls follow the canonical ABI contract: sizes and alignments match the previous call.
    unsafe {
        let p = cabi_realloc(core::ptr::null_mut(), 0, 8, 100);
        assert_eq!(headroom().used(), 100);
        let p = cabi_realloc(p, 100, 8, 300);
        assert_eq!(headroom().used(), 300);
        let p = cabi_realloc(p, 300, 8, 50);
        assert_eq!(headroom().used(), 50);
        let q = cabi_realloc(core::ptr::null_mut(), 0, 16, 1000);
        assert_eq!(headroom().used(), 1050);
        assert_eq!(cabi_realloc(q, 1000, 16, 0) as usize, 16, "zero length returns the alignment and frees");
        assert_eq!(headroom().used(), 50);
        assert_eq!(headroom().used(), 50);
        // The allocator's own count agrees: only the 50 live bytes remain.
        assert_eq!(usage().used, base + 50);
        transfer_free(p, 50, 8);
        transfer_free(core::ptr::null_mut(), 0, 1);
    }
    assert_eq!(headroom().used(), 0, "released to exactly its previous value");
    assert_eq!(usage().used, base);

    // set_headroom: below the current use is refused and changes nothing; an explicit cap sticks.
    // SAFETY: as above.
    let p = unsafe { cabi_realloc(core::ptr::null_mut(), 0, 8, 500) };
    assert_eq!(set_headroom(499), Err(ConfigError::BelowUsage));
    assert_eq!(headroom().cap(), (1 << 25) / 8);
    assert_eq!(set_headroom(500), Ok(()));
    assert_eq!(headroom().cap(), 500);
    raise_ceiling(1 << 26).unwrap();
    assert_eq!(headroom().cap(), 500, "an explicit cap is not recomputed");
    // SAFETY: frees the block allocated above.
    unsafe { transfer_free(p, 500, 8) };
    assert_eq!(set_headroom(0), Ok(()));
    assert_eq!(headroom().cap(), 0);
    // Test support forgets the explicit cap.
    testing::reset(BIG);
    assert_eq!(headroom().cap(), BIG / 8);
    assert_eq!(usage().used, base);
}

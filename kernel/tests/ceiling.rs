//! Gate 5 (ceiling) and gate 10 (transfer hook, non-panicking part).
mod common;
use common::*;
use sentropic_bash_kernel::mem::*;

#[test]
fn ceiling() {
    let base = start();
    let b = root(51, u64::MAX);

    // Configuration errors.
    assert_eq!(raise_ceiling(BIG * 2), Err(ConfigError::NotConfigured));
    let used = usage().used;
    assert_eq!(configure(used - 1), Err(ConfigError::BelowUsage));
    let c = used + 4096;
    assert_eq!(configure(c), Ok(()));
    assert_eq!(configure(c * 2), Err(ConfigError::AlreadyConfigured));
    assert_eq!(usage().ceiling, c);
    assert_eq!(raise_ceiling(c - 1), Err(ConfigError::BelowCurrent));
    assert_eq!(raise_ceiling(c), Ok(()));
    assert_eq!(usage().ceiling, c);

    // An allocation that would pass the ceiling returns Ceiling and leaves the container unchanged.
    let mut v: KVec<u8> = KVec::new_in(&b);
    v.try_reserve_exact(1000).unwrap();
    let snap = (v.len(), v.capacity(), b.used(), usage().used);
    assert_eq!(v.try_reserve_exact(5000), Err(MemError::Ceiling));
    assert_eq!(v.try_push(1), Ok(()));
    let snap = (snap.0 + 1, snap.1, snap.2, snap.3);
    assert_eq!((v.len(), v.capacity(), b.used(), usage().used), snap);
    assert_eq!(KVec::<u8>::with_capacity_in(5000, &b).err(), Some(MemError::Ceiling));
    assert_eq!(b.used(), snap.2, "the refused charge was given back");
    assert_eq!(KBox::try_new_in([0u8; 8192], &b).err().map(|(_, e)| e), Some(MemError::Ceiling));
    assert_eq!(Budget::root(LimitId(9), 1).map(|_| ()), Ok(()), "small allocations still fit");
    let hw = usage().high_water;
    assert!(hw >= usage().used && hw <= c);

    // After raise_ceiling(2 * C) the same operation succeeds.
    assert_eq!(raise_ceiling(2 * c), Ok(()));
    assert_eq!(v.try_reserve_exact(5000), Ok(()));
    assert!(v.capacity() >= 5000);
    assert!(usage().high_water >= hw, "high_water never decreases");
    let hw2 = usage().high_water;
    drop(v);
    assert!(usage().high_water >= hw2);
    assert_eq!(usage().high_water, hw2);

    // The ceiling is also hit by growth through realloc, and a refusal does not charge the budget.
    let mut w: KVec<u64> = KVec::new_in(&b);
    let mut last = 0;
    loop {
        let before = (w.len(), w.capacity(), b.used());
        match w.try_reserve(w.capacity() + 1) {
            Ok(()) => last = w.capacity(),
            Err(e) => {
                assert_eq!(e, MemError::Ceiling);
                assert_eq!(before, (w.len(), w.capacity(), b.used()));
                break;
            }
        }
    }
    assert!(last > 0);
    drop(w);

    transfer();
    drop(b);
    assert_eq!(usage().used, base, "no leak");
}

fn transfer() {
    // Zero-length requests return the alignment value and allocate nothing.
    let before = usage().used;
    for align in [1usize, 2, 4, 8] {
        // SAFETY: a zero-length request involves no block.
        let p = unsafe { cabi_realloc(core::ptr::null_mut(), 0, align, 0) };
        assert_eq!(p as usize, align);
    }
    assert_eq!(usage().used, before);
    // Aligned pointers; growth preserves content; free releases.
    for align in [1usize, 2, 4, 8] {
        for len in [1usize, 3, 8, 64, 1000] {
            // SAFETY: fresh allocation of `len` bytes with a power-of-two alignment.
            let p = unsafe { cabi_realloc(core::ptr::null_mut(), 0, align, len) };
            assert!(!p.is_null());
            assert_eq!(p as usize % align, 0, "aligned to {align}");
            // SAFETY: `p` is a live block of `len` bytes.
            unsafe { core::ptr::write_bytes(p, 0xA5, len) };
            // SAFETY: the block (`p`, `len`, `align`) is live and came from this hook.
            let q = unsafe { cabi_realloc(p, len, align, len * 3 + 1) };
            assert_eq!(q as usize % align, 0);
            // SAFETY: `q` is live with at least `len` initialised bytes.
            let kept = unsafe { core::slice::from_raw_parts(q, len) };
            assert!(kept.iter().all(|&x| x == 0xA5), "content preserved");
            // SAFETY: shrinking a live block; the first bytes are preserved.
            let r = unsafe { cabi_realloc(q, len * 3 + 1, align, 1) };
            // SAFETY: `r` is live with one initialised byte.
            assert_eq!(unsafe { *r }, 0xA5);
            // SAFETY: (`r`, 1, `align`) is the live block from the last call.
            unsafe { transfer_free(r, 1, align) };
        }
    }
    // SAFETY: a zero length is a no-op for any pointer.
    unsafe { transfer_free(core::ptr::null_mut(), 0, 1) };
    // Growing from a block to zero length frees it.
    // SAFETY: fresh block, then freed through the zero-length request.
    unsafe {
        let p = cabi_realloc(core::ptr::null_mut(), 0, 8, 128);
        let z = cabi_realloc(p, 128, 8, 0);
        assert_eq!(z as usize, 8);
    }
    assert_eq!(usage().used, before, "the hook releases what it allocates");
}

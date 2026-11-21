//! Workloads and probes of the allocator tests (spec L1a-2 section 7), exported from the wasm module only
//! under the `mem-test-exports` feature. Integer arguments and results only: a node test instantiates the
//! core module with an empty import object and calls these. Never part of a normal build.
use super::alloc::alloc::{alloc, dealloc, realloc, Layout};
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering::Relaxed};

const SLOTS: usize = 8192;
static LIVE: [AtomicUsize; SLOTS] = [const { AtomicUsize::new(0) }; SLOTS];
static SIZE: [AtomicUsize; SLOTS] = [const { AtomicUsize::new(0) }; SLOTS];
static RNG: AtomicU64 = AtomicU64::new(0x9E37_79B9_7F4A_7C15);

fn rand() -> u64 {
    let mut x = RNG.load(Relaxed);
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    RNG.store(x, Relaxed);
    x
}

fn below(n: usize) -> usize {
    (rand() % n as u64) as usize
}

fn layout(size: usize) -> Layout {
    Layout::from_size_align(size, 8).unwrap()
}

/// Allocates `size` bytes into slot `i`; false when the allocator refuses.
fn put(i: usize, size: usize) -> bool {
    // SAFETY: `size` is non-zero (callers pass at least 1) and the layout is valid.
    let p = unsafe { alloc(layout(size)) } as usize;
    if p == 0 {
        return false;
    }
    // SAFETY: the block holds at least one byte.
    unsafe { (p as *mut u8).write(size as u8) };
    LIVE[i].store(p, Relaxed);
    SIZE[i].store(size, Relaxed);
    true
}

fn take(i: usize) {
    let p = LIVE[i].swap(0, Relaxed);
    if p != 0 {
        // SAFETY: slot `i` holds a live block allocated with exactly this size and alignment 8.
        unsafe { dealloc(p as *mut u8, layout(SIZE[i].load(Relaxed))) };
    }
}

fn free_all() {
    for i in 0..SLOTS {
        take(i);
    }
}

fn pick_size(w: u32) -> usize {
    match w {
        1 => 16 + below(4096 - 16 + 1),
        _ => {
            if below(10) == 0 {
                65_536 + below(1_048_576 - 65_536 + 1)
            } else {
                16 + below(256 - 16 + 1)
            }
        }
    }
}

/// W1 (uniform) and W2 (bimodal): fill to `target` live blocks, then 10 000 cycles of allocate or free with
/// equal probability. Returns 0, or 1 when an allocation was refused.
fn churn(w: u32, target: usize) -> u32 {
    let mut count = 0usize;
    let mut order = [0usize; SLOTS];
    let mut free_slots = [0usize; SLOTS];
    let mut nfree = SLOTS;
    for (k, s) in free_slots.iter_mut().enumerate() {
        *s = k;
    }
    let mut ok = true;
    for step in 0..(target + 10_000) {
        let grow = step < target || below(2) == 0;
        if grow && nfree > 0 {
            nfree -= 1;
            let slot = free_slots[nfree];
            if !put(slot, pick_size(w)) {
                free_slots[nfree] = slot;
                nfree += 1;
                ok = false;
                break;
            }
            order[count] = slot;
            count += 1;
        } else if count > 0 {
            let j = below(count);
            let slot = order[j];
            order[j] = order[count - 1];
            count -= 1;
            take(slot);
            free_slots[nfree] = slot;
            nfree += 1;
        }
    }
    free_all();
    if ok {
        0
    } else {
        1
    }
}

/// W3: eight buffers grown by `realloc` in steps of 1.5 times from 64 bytes up to the largest size that is
/// at most 8 MiB (about 7.8 MiB), round-robin.
fn growing() -> u32 {
    let mut size = [64usize; 8];
    let mut ptr = [0usize; 8];
    for p in ptr.iter_mut() {
        // SAFETY: valid non-zero layout.
        *p = unsafe { alloc(layout(64)) } as usize;
        if *p == 0 {
            return 1;
        }
    }
    let mut ok = true;
    'outer: while size.iter().any(|&s| (s * 3).div_ceil(2) <= 8 << 20) {
        for k in 0..8 {
            let new = (size[k] * 3).div_ceil(2);
            if new > 8 << 20 {
                continue;
            }
            // SAFETY: `ptr[k]` is a live block of `size[k]` bytes with alignment 8.
            let q = unsafe { realloc(ptr[k] as *mut u8, layout(size[k]), new) } as usize;
            if q == 0 {
                ok = false;
                break 'outer;
            }
            ptr[k] = q;
            size[k] = new;
            // SAFETY: the block holds `new >= 1` bytes.
            unsafe { (q as *mut u8).write(k as u8) };
        }
    }
    for k in 0..8 {
        // SAFETY: `ptr[k]` is a live block of `size[k]` bytes with alignment 8.
        unsafe { dealloc(ptr[k] as *mut u8, layout(size[k])) };
    }
    u32::from(!ok)
}

/// W4 (sawtooth): 4 096 blocks of 4 KiB, free every other one, 2 048 blocks of 8 KiB.
fn sawtooth() -> u32 {
    let mut ok = true;
    for i in 0..4096 {
        ok &= put(i, 4096);
    }
    for i in (0..4096).step_by(2) {
        take(i);
    }
    for i in 0..2048 {
        ok &= put(4096 + i, 8192);
    }
    free_all();
    u32::from(!ok)
}

/// Runs workload `w` (1 uniform, 2 bimodal, 3 growing buffers, 4 sawtooth); `seed` 0 is the fixed default.
#[no_mangle]
pub extern "C" fn t_run(w: u32, seed: u32) -> u32 {
    RNG.store(0x9E37_79B9_7F4A_7C15 ^ u64::from(w).wrapping_mul(0x1000_0000_01B3) ^ u64::from(seed).wrapping_mul(0xD6E8_FEB8_6659_FD93), Relaxed);
    match w {
        1 => churn(1, 4096),
        2 => churn(2, 2000),
        3 => growing(),
        _ => sawtooth(),
    }
}

/// After a workload freed everything: allocate 99% of the footprint minus the reserve and report whether
/// it fit without growing the footprint (1) or not (0).
#[no_mangle]
pub extern "C" fn t_full_return() -> u32 {
    let before = super::footprint();
    let size = ((before - 65_536) / 100 * 99) as usize;
    // SAFETY: valid non-zero layout.
    let p = unsafe { alloc(layout(size)) };
    let ok = !p.is_null() && super::footprint() == before;
    if !p.is_null() {
        // SAFETY: `p` was just allocated with this layout.
        unsafe { dealloc(p, layout(size)) };
    }
    u32::from(ok)
}

/// Pairs of `size`-byte allocate/free, `n` times (the speed gate).
#[no_mangle]
pub extern "C" fn t_pairs(n: u32, size: u32) -> u32 {
    let l = layout(size as usize);
    let mut sum = 0usize;
    for _ in 0..n {
        // SAFETY: valid non-zero layout; the block is freed right away with the same layout.
        unsafe {
            let p = alloc(l);
            sum = sum.wrapping_add(p as usize);
            dealloc(p, l);
        }
    }
    sum as u32
}

#[no_mangle]
pub extern "C" fn t_used() -> u32 {
    super::usage().used as u32
}
#[no_mangle]
pub extern "C" fn t_peak() -> u32 {
    super::usage().high_water as u32
}
#[no_mangle]
pub extern "C" fn t_footprint() -> u32 {
    super::footprint() as u32
}
#[no_mangle]
pub extern "C" fn t_footprint_hw() -> u32 {
    super::footprint_high_water() as u32
}
#[no_mangle]
pub extern "C" fn t_configure(bytes: u32) -> u32 {
    u32::from(super::configure(u64::from(bytes)).is_err())
}
#[no_mangle]
pub extern "C" fn t_raise(bytes: u32) -> u32 {
    u32::from(super::raise_ceiling(u64::from(bytes)).is_err())
}

/// Raw allocation: the address, or 0.
#[no_mangle]
pub extern "C" fn t_alloc(size: u32, align: u32) -> u32 {
    let Ok(l) = Layout::from_size_align(size as usize, align as usize) else { return 0 };
    // SAFETY: the layout is valid and non-zero sized for the callers of this test hook.
    unsafe { alloc(l) as usize as u32 }
}

#[no_mangle]
pub extern "C" fn t_free(ptr: u32, size: u32, align: u32) {
    // SAFETY: the caller passes a live block with exactly this size and alignment.
    unsafe { dealloc(ptr as usize as *mut u8, Layout::from_size_align_unchecked(size as usize, align as usize)) };
}

#[no_mangle]
pub extern "C" fn t_realloc(ptr: u32, old: u32, align: u32, new: u32) -> u32 {
    // SAFETY: the caller passes a live block with exactly this size and alignment, and a non-zero `new`.
    unsafe { realloc(ptr as usize as *mut u8, Layout::from_size_align_unchecked(old as usize, align as usize), new as usize) as usize as u32 }
}

/// Allocates `size`-byte blocks until the allocator refuses; returns how many it got (kept live in the slots).
#[no_mangle]
pub extern "C" fn t_fill(size: u32) -> u32 {
    let mut n = 0;
    while n < SLOTS && put(n, size as usize) {
        n += 1;
    }
    n as u32
}

/// Frees what `t_fill` kept.
#[no_mangle]
pub extern "C" fn t_unfill() {
    free_all();
}

/// Probe of the reserve with the heap already filled: bit 0 ordinary request refused, bit 1 served inside the
/// scope, bit 2 served inside a nested scope, bit 3 still served after the nested scope returned, bit 4
/// ordinary request refused again after the scope closed.
#[no_mangle]
pub extern "C" fn t_reserve_probe(size: u32) -> u32 {
    let l = layout(size as usize);
    let try_one = || {
        // SAFETY: valid non-zero layout; a block obtained is freed at once.
        let p = unsafe { alloc(l) };
        if !p.is_null() {
            // SAFETY: just allocated with this layout.
            unsafe { dealloc(p, l) };
        }
        !p.is_null()
    };
    let mut bits = 0;
    bits |= u32::from(!try_one());
    super::reserve_scope(|| {
        bits |= u32::from(try_one()) << 1;
        super::reserve_scope(|| {
            bits |= u32::from(try_one()) << 2;
        });
        bits |= u32::from(try_one()) << 3;
    });
    bits |= u32::from(!try_one()) << 4;
    bits
}

#[no_mangle]
pub extern "C" fn t_cabi(old_ptr: u32, old_len: u32, align: u32, new_len: u32) -> u32 {
    // SAFETY: the caller follows the canonical ABI contract for the hook.
    unsafe { super::cabi_realloc(old_ptr as usize as *mut u8, old_len as usize, align as usize, new_len as usize) as usize as u32 }
}

#[no_mangle]
pub extern "C" fn t_transfer_free(ptr: u32, len: u32, align: u32) {
    // SAFETY: the caller passes a live block from `t_cabi`.
    unsafe { super::transfer_free(ptr as usize as *mut u8, len as usize, align as usize) };
}

#[no_mangle]
pub extern "C" fn t_headroom_used() -> u32 {
    super::headroom().used() as u32
}
#[no_mangle]
pub extern "C" fn t_headroom_cap() -> u32 {
    super::headroom().cap() as u32
}
#[no_mangle]
pub extern "C" fn t_set_headroom(bytes: u32) -> u32 {
    u32::from(super::set_headroom(u64::from(bytes)).is_err())
}
